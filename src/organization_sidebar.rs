//! Contextual, docked organization tree for the Herdr project workspace.

use std::collections::{BTreeSet, HashMap};
use std::fs::{self, OpenOptions};
use std::io::{self, IsTerminal, Write};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use crossterm::cursor::{Hide, MoveTo, Show};
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::execute;
use crossterm::style::{Attribute, Color, ResetColor, SetAttribute, SetForegroundColor};
use crossterm::terminal::{
    self, BeginSynchronizedUpdate, Clear, ClearType, EndSynchronizedUpdate, EnterAlternateScreen,
    LeaveAlternateScreen,
};
use serde::{Deserialize, Serialize};
use unicode_width::UnicodeWidthStr;

use crate::herdr::{Herdr, Pane};
use crate::organizations::{self, TreeEntry};
use crate::organizations_ui;
use crate::paths::Ctx;
use crate::project::{self, Project, Status};
use crate::thread::{self as thread_model, Group, NodeRole};
use crate::threads;

pub const ACTION_ID: &str = "organization-sidebar";
pub const AUTO_OPEN_ACTION_ID: &str = "organization-sidebar-auto-open";

const PROJECT_ENV: &str = "HERDR_ORGANIZATIONS_PROJECT";
const WORKSPACE_ENV: &str = "HERDR_ORGANIZATIONS_WORKSPACE";
const CONFIG_FILE: &str = "organization-sidebar.json";
const LOCK_FILE_PREFIX: &str = ".organization-sidebar-";
const METADATA_SOURCE: &str = crate::herdr::SOURCE;
const TOKEN_ID: &str = "org_sidebar";
const TOKEN_PROJECT: &str = "org_project";
const TOKEN_WORKSPACE: &str = "org_workspace";
const TOKEN_HEARTBEAT: &str = "org_heartbeat";
const TOKEN_TTL: Duration = Duration::from_secs(60);
const TOKEN_REFRESH: Duration = Duration::from_secs(10);
const VIEW_REFRESH: Duration = Duration::from_secs(5);
const LOCK_WAIT: Duration = Duration::from_millis(50);
const LOCK_ATTEMPTS: usize = 40;
const LOCK_STALE_AFTER: Duration = Duration::from_secs(30);
const MAX_RENDERED_NODES: usize = 1_000;
const SETTINGS_COUNT: usize = 8;
const SETTINGS_VALUE_COLUMN: usize = 24;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum DockSide {
    Left,
    #[default]
    Right,
}

impl DockSide {
    fn label(self) -> &'static str {
        match self {
            DockSide::Left => "Left",
            DockSide::Right => "Right",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct SidebarSettings {
    pub dock_side: DockSide,
    pub width_percent: u8,
    pub focus_on_open: bool,
    pub auto_open: bool,
    pub show_resolved: bool,
    pub show_status: bool,
    pub show_role: bool,
    pub strict_toggle: bool,
}

impl Default for SidebarSettings {
    fn default() -> Self {
        Self {
            dock_side: DockSide::Right,
            width_percent: 30,
            focus_on_open: false,
            auto_open: false,
            show_resolved: false,
            show_status: true,
            show_role: true,
            strict_toggle: true,
        }
    }
}

impl SidebarSettings {
    fn normalize(&mut self) {
        self.width_percent = self.width_percent.clamp(15, 50);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToggleResult {
    Opened,
    Closed,
    Focused,
    AlreadyOpen,
}

enum Operation {
    Toggle,
    Ensure,
}

fn config_dir(ctx: &Ctx) -> Result<PathBuf> {
    ctx.env
        .var("HERDR_PLUGIN_CONFIG_DIR")
        .map(PathBuf::from)
        .context("HERDR_PLUGIN_CONFIG_DIR is not set for this Herdr plugin")
}

fn settings_path(ctx: &Ctx) -> Result<PathBuf> {
    Ok(config_dir(ctx)?.join(CONFIG_FILE))
}

pub fn load_settings(ctx: &Ctx) -> Result<SidebarSettings> {
    let path = settings_path(ctx)?;
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(SidebarSettings::default());
        }
        Err(error) => {
            return Err(error).with_context(|| format!("could not read {}", path.display()));
        }
    };
    let mut settings: SidebarSettings = serde_json::from_str(&text)
        .with_context(|| format!("{} does not parse", path.display()))?;
    settings.normalize();
    Ok(settings)
}

pub fn save_settings(ctx: &Ctx, settings: &SidebarSettings) -> Result<()> {
    let path = settings_path(ctx)?;
    let dir = path.parent().context("plugin config path has no parent")?;
    fs::create_dir_all(dir).with_context(|| format!("could not create {}", dir.display()))?;
    let mut settings = settings.clone();
    settings.normalize();
    project::write_json(&path, &settings)
}

pub fn toggle(ctx: &Ctx, slug: &str, workspace: &str, source_pane: &str) -> Result<ToggleResult> {
    operate(ctx, slug, workspace, source_pane, Operation::Toggle)
}

pub fn ensure_auto_open(
    ctx: &Ctx,
    slug: Option<&str>,
    workspace: &str,
    source_pane: &str,
) -> Result<ToggleResult> {
    if slug.is_none() || workspace.is_empty() || ctx.env.var("HERDR_PLUGIN_CONFIG_DIR").is_none() {
        return Ok(ToggleResult::AlreadyOpen);
    }
    let settings = load_settings(ctx)?;
    if !settings.auto_open {
        return Ok(ToggleResult::AlreadyOpen);
    }
    operate_with_settings(
        ctx,
        slug.expect("checked above"),
        workspace,
        source_pane,
        Operation::Ensure,
        settings,
    )
}

fn operate(
    ctx: &Ctx,
    slug: &str,
    workspace: &str,
    source_pane: &str,
    operation: Operation,
) -> Result<ToggleResult> {
    let settings = load_settings(ctx)?;
    operate_with_settings(ctx, slug, workspace, source_pane, operation, settings)
}

fn operate_with_settings(
    ctx: &Ctx,
    slug: &str,
    workspace: &str,
    source_pane: &str,
    operation: Operation,
    settings: SidebarSettings,
) -> Result<ToggleResult> {
    project::validate_slug(slug)?;
    if workspace.is_empty() {
        bail!("Herdr did not provide a workspace for the organization sidebar");
    }
    let config_dir = config_dir(ctx)?;
    let _lock = OperationLock::acquire(&config_dir, workspace)?;
    let socket = ctx
        .env
        .var("HERDR_SOCKET_PATH")
        .context("HERDR_SOCKET_PATH is not set for this Herdr action")?;
    let herdr = Herdr::new(ctx.env.herdr_bin(), socket, ctx.runner);
    let panes = herdr
        .pane_list()
        .map_err(|error| anyhow::anyhow!("{error}"))?;
    let mut own_panes = panes
        .iter()
        .filter(|pane| sidebar_pane(pane, slug, workspace))
        .cloned()
        .collect::<Vec<_>>();
    own_panes.sort_by(|left, right| left.pane_id.cmp(&right.pane_id));

    if !own_panes.is_empty() {
        match operation {
            Operation::Ensure => return Ok(ToggleResult::AlreadyOpen),
            Operation::Toggle
                if settings.strict_toggle || own_panes.iter().any(|pane| pane.focused) =>
            {
                for pane in own_panes {
                    close_sidebar_pane(&herdr, &pane.pane_id)?;
                }
                return Ok(ToggleResult::Closed);
            }
            Operation::Toggle if settings.focus_on_open => {
                herdr
                    .plugin_pane_focus(&own_panes[0].pane_id)
                    .map_err(|error| anyhow::anyhow!("{error}"))?;
                return Ok(ToggleResult::Focused);
            }
            Operation::Toggle => return Ok(ToggleResult::AlreadyOpen),
        }
    }

    let source = panes
        .iter()
        .find(|pane| pane.pane_id == source_pane && pane.workspace_id == workspace)
        .or_else(|| {
            panes
                .iter()
                .find(|pane| pane.workspace_id == workspace && pane.focused)
        })
        .or_else(|| panes.iter().find(|pane| pane.workspace_id == workspace))
        .with_context(|| format!("no live Herdr pane was found in workspace `{workspace}`"))?;
    let ratio = match settings.dock_side {
        DockSide::Right => 1.0 - f64::from(settings.width_percent) / 100.0,
        DockSide::Left => f64::from(settings.width_percent) / 100.0,
    };
    let created = herdr
        .pane_split(&source.pane_id, "right", ratio, &source.cwd)
        .map_err(|error| anyhow::anyhow!("{error}"))?;

    if settings.dock_side == DockSide::Left
        && let Err(error) = herdr.pane_swap(&created.pane_id, &source.pane_id)
    {
        let _ = herdr.pane_close(&created.pane_id);
        return Err(anyhow::anyhow!("{error}"));
    }
    let _ = herdr.pane_rename(&created.pane_id, "Organization");
    let command = launch_argv(ctx, &created.pane_id, slug, workspace, &config_dir, socket)?;
    if let Err(error) = herdr.pane_run(&created.pane_id, &command) {
        let _ = herdr.pane_close(&created.pane_id);
        return Err(anyhow::anyhow!("{error}"));
    }
    if let Err(error) = report_identity_after_split(&herdr, &created.pane_id, slug, workspace) {
        let _ = herdr.pane_close(&created.pane_id);
        return Err(anyhow::anyhow!("{error}"));
    }

    let direction = match settings.dock_side {
        DockSide::Left => "left",
        DockSide::Right => "right",
    };
    if settings.focus_on_open {
        herdr
            .pane_focus_direction(&source.pane_id, direction)
            .map_err(|error| anyhow::anyhow!("{error}"))?;
    } else if settings.dock_side == DockSide::Left {
        herdr
            .pane_focus_direction(&created.pane_id, "right")
            .map_err(|error| anyhow::anyhow!("{error}"))?;
    }
    Ok(ToggleResult::Opened)
}

fn sidebar_pane(pane: &Pane, slug: &str, workspace: &str) -> bool {
    pane.workspace_id == workspace
        && pane
            .tokens
            .get(TOKEN_ID)
            .and_then(serde_json::Value::as_str)
            == Some("v1")
        && pane
            .tokens
            .get(TOKEN_PROJECT)
            .and_then(serde_json::Value::as_str)
            == Some(slug)
        && pane
            .tokens
            .get(TOKEN_WORKSPACE)
            .and_then(serde_json::Value::as_str)
            == Some(workspace)
}

fn close_sidebar_pane(herdr: &Herdr<'_>, pane_id: &str) -> Result<()> {
    match herdr.pane_close(pane_id) {
        Ok(()) => Ok(()),
        Err(error) if error.code == "pane_not_found" => Ok(()),
        Err(error) => Err(anyhow::anyhow!("{error}")),
    }
}

fn report_identity(herdr: &Herdr<'_>, pane: &str, slug: &str, workspace: &str) -> Result<()> {
    let heartbeat = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .to_string();
    herdr
        .pane_report_tokens_from(
            pane,
            METADATA_SOURCE,
            &[
                (TOKEN_ID, "v1"),
                (TOKEN_PROJECT, slug),
                (TOKEN_WORKSPACE, workspace),
                (TOKEN_HEARTBEAT, &heartbeat),
            ],
            TOKEN_TTL,
        )
        .map_err(|error| anyhow::anyhow!("{error}"))
}

fn report_identity_after_split(
    herdr: &Herdr<'_>,
    pane: &str,
    slug: &str,
    workspace: &str,
) -> Result<()> {
    let mut last_error = None;
    for _ in 0..10 {
        match report_identity(herdr, pane, slug, workspace) {
            Ok(()) => return Ok(()),
            Err(error) => last_error = Some(error),
        }
        // A split response can arrive before the new pane is visible to the
        // metadata registry. Refreshing the pane inventory synchronizes that
        // registry before the next bounded retry.
        let _ = herdr.pane_list();
        thread::sleep(Duration::from_millis(25));
    }
    Err(last_error.expect("identity reporting was attempted"))
}

fn launch_argv(
    ctx: &Ctx,
    pane: &str,
    slug: &str,
    workspace: &str,
    config_dir: &Path,
    socket: &str,
) -> Result<Vec<String>> {
    let binary = std::env::current_exe().context("could not locate herdr-organizations binary")?;
    let mut assignments = vec![
        ("HERDR_PANE_ID", pane.to_string()),
        ("HERDR_WORKSPACE_ID", workspace.to_string()),
        (PROJECT_ENV, slug.to_string()),
        (WORKSPACE_ENV, workspace.to_string()),
        (
            "HERDR_PLUGIN_CONFIG_DIR",
            config_dir.to_string_lossy().into_owned(),
        ),
        ("HERDR_SOCKET_PATH", socket.to_string()),
    ];
    if let Some(bin) = ctx.env.var("HERDR_BIN_PATH") {
        assignments.push(("HERDR_BIN_PATH", bin.to_string()));
    }
    let mut command = vec!["env".to_string()];
    command.extend(
        assignments
            .into_iter()
            .map(|(key, value)| format!("{key}={value}")),
    );
    command.extend([
        binary.to_string_lossy().into_owned(),
        "--root".to_string(),
        ctx.root.to_string_lossy().into_owned(),
        "pane".to_string(),
        "organization-sidebar".to_string(),
    ]);
    Ok(command)
}

struct OperationLock {
    path: PathBuf,
}

impl OperationLock {
    fn acquire(config_dir: &Path, workspace: &str) -> Result<Self> {
        fs::create_dir_all(config_dir)
            .with_context(|| format!("could not create {}", config_dir.display()))?;
        let suffix = workspace
            .chars()
            .filter(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
            .collect::<String>();
        let path = config_dir.join(format!("{LOCK_FILE_PREFIX}{suffix}.lock"));
        for _ in 0..LOCK_ATTEMPTS {
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(mut file) => {
                    writeln!(file, "{}", std::process::id())?;
                    return Ok(Self { path });
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    let stale = fs::metadata(&path)
                        .and_then(|metadata| metadata.modified())
                        .ok()
                        .and_then(|modified| modified.elapsed().ok())
                        .is_some_and(|age| age > LOCK_STALE_AFTER);
                    if stale {
                        let _ = fs::remove_file(&path);
                        continue;
                    }
                    thread::sleep(LOCK_WAIT);
                }
                Err(error) => {
                    return Err(error)
                        .with_context(|| format!("could not create {}", path.display()));
                }
            }
        }
        bail!("the organization sidebar is already changing in workspace `{workspace}`")
    }
}

impl Drop for OperationLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

struct TerminalGuard;

impl TerminalGuard {
    fn enter() -> Result<Self> {
        terminal::enable_raw_mode().context("could not enable sidebar input")?;
        let guard = Self;
        if let Err(error) = execute!(io::stdout(), EnterAlternateScreen, Hide) {
            let _ = terminal::disable_raw_mode();
            return Err(error).context("could not open organization sidebar screen");
        }
        Ok(guard)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = execute!(
            io::stdout(),
            Show,
            LeaveAlternateScreen,
            ResetColor,
            SetAttribute(Attribute::Reset)
        );
        let _ = terminal::disable_raw_mode();
    }
}

#[derive(Debug, Clone)]
struct TreeView {
    project: Project,
    entries: Vec<TreeEntry>,
    groups: HashMap<String, Group>,
    omitted_nodes: usize,
}

#[derive(Debug, Clone)]
enum Screen {
    Tree {
        view: TreeView,
        collapsed: BTreeSet<String>,
        root_collapsed: bool,
        selected: usize,
    },
    Settings {
        selected: usize,
    },
}

#[derive(Clone, Copy)]
enum VisibleRow<'a> {
    Root,
    Entry(&'a TreeEntry),
}

pub fn run(ctx: &Ctx) -> Result<()> {
    let slug = ctx
        .env
        .var(PROJECT_ENV)
        .context("the organization sidebar was not given a project context")?;
    project::validate_slug(slug)?;
    let workspace = ctx
        .env
        .var(WORKSPACE_ENV)
        .context("the organization sidebar was not given a workspace context")?;
    let pane_id = ctx
        .env
        .var("HERDR_PANE_ID")
        .context("Herdr did not provide the organization sidebar pane id")?;
    let mut settings = load_settings(ctx)?;
    let mut view = load_view(ctx, slug, settings.show_resolved)?;
    let mut screen = Screen::Tree {
        view: view.clone(),
        collapsed: BTreeSet::new(),
        root_collapsed: false,
        selected: 0,
    };

    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        print_snapshot(&view, &settings);
        return Ok(());
    }

    let socket = ctx
        .env
        .var("HERDR_SOCKET_PATH")
        .context("HERDR_SOCKET_PATH is not set for the organization sidebar")?;
    let herdr = Herdr::new(ctx.env.herdr_bin(), socket, ctx.runner);
    let _ = report_identity(&herdr, pane_id, slug, workspace);
    let guard = TerminalGuard::enter()?;
    let mut message = String::new();
    let mut last_refresh = Instant::now();
    let mut last_heartbeat = Instant::now();
    let mut last_size = None;
    let mut dirty = true;
    let mut rendered_frame = Vec::new();

    loop {
        let (width, height) = terminal::size().unwrap_or((40, 24));
        let size = (width as usize, height as usize);
        if last_size != Some(size) {
            last_size = Some(size);
            dirty = true;
            rendered_frame.clear();
        }
        if dirty {
            render_incremental(
                &mut io::stdout(),
                &mut rendered_frame,
                &screen,
                &settings,
                &message,
                size.0,
                size.1,
            )?;
            dirty = false;
        }

        if last_heartbeat.elapsed() >= TOKEN_REFRESH {
            let _ = report_identity(&herdr, pane_id, slug, workspace);
            last_heartbeat = Instant::now();
        }
        if last_refresh.elapsed() >= VIEW_REFRESH {
            if let Screen::Tree {
                view: tree_view,
                selected,
                collapsed,
                root_collapsed,
            } = &mut screen
            {
                match load_view(ctx, slug, settings.show_resolved) {
                    Ok(refreshed) => {
                        dirty |= tree_view_changed(tree_view, &refreshed);
                        *tree_view = refreshed.clone();
                        view = refreshed.clone();
                        *selected = (*selected).min(
                            visible_rows(&refreshed, collapsed, *root_collapsed)
                                .len()
                                .saturating_sub(1),
                        );
                    }
                    Err(error) => {
                        let refresh_message = format!("Refresh failed: {error:#}");
                        dirty |= message != refresh_message;
                        message = refresh_message;
                    }
                }
            }
            last_refresh = Instant::now();
        }
        let poll_timeout = VIEW_REFRESH
            .saturating_sub(last_refresh.elapsed())
            .min(TOKEN_REFRESH.saturating_sub(last_heartbeat.elapsed()));
        if !event::poll(poll_timeout)? {
            continue;
        }
        let key = match event::read()? {
            Event::Key(key) => key,
            Event::Resize(_, _) => {
                dirty = true;
                continue;
            }
            _ => continue,
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        dirty = true;
        message.clear();
        match (&mut screen, key.code) {
            (_, KeyCode::Char('q')) => break,
            (Screen::Settings { .. }, KeyCode::Esc) => {
                screen = Screen::Tree {
                    view: view.clone(),
                    collapsed: BTreeSet::new(),
                    root_collapsed: false,
                    selected: 0,
                };
            }
            (Screen::Tree { .. }, KeyCode::Esc) => break,
            (Screen::Tree { .. }, KeyCode::Char('s' | 'g')) => {
                screen = Screen::Settings { selected: 0 };
            }
            (Screen::Settings { selected }, KeyCode::Up | KeyCode::Char('k')) => {
                *selected = selected.saturating_sub(1);
            }
            (Screen::Settings { selected }, KeyCode::Down | KeyCode::Char('j')) => {
                *selected = selected.saturating_add(1).min(SETTINGS_COUNT - 1);
            }
            (Screen::Settings { selected }, KeyCode::Left) => {
                modify_setting(&mut settings, *selected, -1);
                save_settings(ctx, &settings)?;
                if *selected == 5 {
                    view = load_view(ctx, slug, settings.show_resolved)?;
                }
            }
            (
                Screen::Settings { selected },
                KeyCode::Right | KeyCode::Enter | KeyCode::Char(' '),
            ) => {
                let direction = if key.code == KeyCode::Right { 1 } else { 0 };
                modify_setting(&mut settings, *selected, direction);
                save_settings(ctx, &settings)?;
                if *selected == 5 {
                    view = load_view(ctx, slug, settings.show_resolved)?;
                }
            }
            (Screen::Tree { selected, .. }, KeyCode::Up | KeyCode::Char('k')) => {
                *selected = selected.saturating_sub(1);
            }
            (
                Screen::Tree {
                    selected,
                    view,
                    collapsed,
                    root_collapsed,
                },
                KeyCode::Down | KeyCode::Char('j'),
            ) => {
                let count = visible_rows(view, collapsed, *root_collapsed).len();
                *selected = selected.saturating_add(1).min(count.saturating_sub(1));
            }
            (
                Screen::Tree {
                    view,
                    collapsed,
                    root_collapsed,
                    selected,
                },
                KeyCode::Char(' '),
            ) => {
                toggle_collapse(view, collapsed, root_collapsed, selected);
            }
            (
                Screen::Tree {
                    view,
                    collapsed,
                    root_collapsed,
                    selected,
                },
                KeyCode::Enter,
            ) => {
                let rows = visible_rows(view, collapsed, *root_collapsed);
                let selected_row = rows.get(*selected).copied();
                let node = match selected_row {
                    Some(VisibleRow::Root) => None,
                    Some(VisibleRow::Entry(entry)) => Some(&entry.thread),
                    None => continue,
                };
                match organizations_ui::focus_node(ctx, &view.project, node) {
                    Ok(pane) => message = format!("Focused {pane}"),
                    Err(error) => message = format!("Could not open selection: {error:#}"),
                }
            }
            (Screen::Tree { .. }, KeyCode::Char('r')) => {
                match load_view(ctx, slug, settings.show_resolved) {
                    Ok(refreshed) => {
                        view = refreshed.clone();
                        screen = Screen::Tree {
                            view: refreshed,
                            collapsed: BTreeSet::new(),
                            root_collapsed: false,
                            selected: 0,
                        };
                    }
                    Err(error) => message = format!("Refresh failed: {error:#}"),
                }
            }
            _ => {}
        }
        if let Screen::Tree { view: tree, .. } = &screen {
            view = tree.clone();
        }
        last_refresh = Instant::now();
    }

    drop(guard);
    close_sidebar_pane(&herdr, pane_id)
}

fn tree_view_changed(current: &TreeView, refreshed: &TreeView) -> bool {
    current.project.root != refreshed.project.root
        || current.project.slug != refreshed.project.slug
        || current.entries != refreshed.entries
        || current.groups != refreshed.groups
        || current.omitted_nodes != refreshed.omitted_nodes
        || project_label(&current.project) != project_label(&refreshed.project)
}

fn load_view(ctx: &Ctx, slug: &str, show_resolved: bool) -> Result<TreeView> {
    let project = Project::load(&ctx.root, slug)?;
    let mut entries = organizations_ui::tree_entries(&project, show_resolved)?;
    let omitted_nodes = entries.len().saturating_sub(MAX_RENDERED_NODES);
    entries.truncate(MAX_RENDERED_NODES);
    let groups = threads::rows(ctx, &project)
        .into_iter()
        .map(|row| (row.thread.id, row.group))
        .collect();
    Ok(TreeView {
        project,
        entries,
        groups,
        omitted_nodes,
    })
}

fn print_snapshot(view: &TreeView, settings: &SidebarSettings) {
    let width = terminal::size()
        .map(|(width, _)| width as usize)
        .unwrap_or(40);
    let collapsed = BTreeSet::new();
    println!(
        "{}",
        organizations_ui::fit_terminal_row(&project_label(&view.project), width)
    );
    for row in visible_rows(view, &collapsed, false).iter() {
        println!(
            "{}",
            organizations_ui::fit_terminal_row(
                &row_text(view, row, settings, &collapsed, false),
                width
            )
        );
    }
    println!(
        "{}",
        organizations_ui::fit_terminal_row("⚙ Settings [s]  ·  q close", width)
    );
}

fn project_label(project: &Project) -> String {
    project
        .read_project_md()
        .map(|(settings, _)| project::display_name(&settings.name, &project.slug))
        .unwrap_or_else(|_| project::humanize(&project.slug))
}

fn visible_rows<'a>(
    view: &'a TreeView,
    collapsed: &BTreeSet<String>,
    root_collapsed: bool,
) -> Vec<VisibleRow<'a>> {
    let mut rows = vec![VisibleRow::Root];
    if root_collapsed {
        return rows;
    }
    let mut hidden_depth: Option<usize> = None;
    for entry in &view.entries {
        if hidden_depth.is_some_and(|depth| entry.depth > depth) {
            continue;
        }
        hidden_depth = None;
        rows.push(VisibleRow::Entry(entry));
        if collapsed.contains(&entry.thread.id) {
            hidden_depth = Some(entry.depth);
        }
    }
    rows
}

fn toggle_collapse(
    view: &TreeView,
    collapsed: &mut BTreeSet<String>,
    root_collapsed: &mut bool,
    selected: &usize,
) {
    let rows = visible_rows(view, collapsed, *root_collapsed);
    match rows.get(*selected).copied() {
        Some(VisibleRow::Root) if !view.entries.is_empty() => *root_collapsed = !*root_collapsed,
        Some(VisibleRow::Entry(entry)) if entry.thread.role == NodeRole::Coordinator => {
            let has_children = view
                .entries
                .iter()
                .any(|candidate| organizations::parent_id(&candidate.thread) == entry.thread.id);
            if has_children && !collapsed.insert(entry.thread.id.clone()) {
                collapsed.remove(&entry.thread.id);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
fn render(
    writer: &mut impl Write,
    screen: &Screen,
    settings: &SidebarSettings,
    message: &str,
    width: usize,
    height: usize,
) -> Result<()> {
    render_incremental(
        writer,
        &mut Vec::new(),
        screen,
        settings,
        message,
        width,
        height,
    )
}

fn render_incremental(
    writer: &mut impl Write,
    previous: &mut Vec<Vec<u8>>,
    screen: &Screen,
    settings: &SidebarSettings,
    message: &str,
    width: usize,
    height: usize,
) -> Result<()> {
    let frame = build_frame(screen, settings, message, width, height)?;
    if *previous == frame {
        return Ok(());
    }
    execute!(writer, BeginSynchronizedUpdate)?;
    for (row, content) in frame.iter().enumerate() {
        if previous.get(row) == Some(content) {
            continue;
        }
        execute!(
            writer,
            MoveTo(0, row.min(u16::MAX as usize) as u16),
            ResetColor,
            SetAttribute(Attribute::Reset),
            Clear(ClearType::CurrentLine)
        )?;
        writer.write_all(content)?;
    }
    execute!(
        writer,
        ResetColor,
        SetAttribute(Attribute::Reset),
        EndSynchronizedUpdate
    )?;
    writer.flush()?;
    *previous = frame;
    Ok(())
}

fn build_frame(
    screen: &Screen,
    settings: &SidebarSettings,
    message: &str,
    width: usize,
    height: usize,
) -> Result<Vec<Vec<u8>>> {
    let mut frame = vec![Vec::new(); height.max(1)];
    match screen {
        Screen::Tree {
            view,
            collapsed,
            root_collapsed,
            selected,
        } => {
            set_accent_row(&mut frame, 0, &project_label(&view.project), width);
            let rows = visible_rows(view, collapsed, *root_collapsed);
            let node_count = rows.len();
            set_muted_row(
                &mut frame,
                1,
                &format!(
                    "Organization  ·  {node_count} {}",
                    if node_count == 1 { "node" } else { "nodes" }
                ),
                width,
            );
            let body_capacity = height.saturating_sub(5).max(1);
            let range = visible_range(rows.len(), *selected, body_capacity);
            let row_context = TreeRowContext {
                view,
                settings,
                collapsed,
                root_collapsed: *root_collapsed,
                width,
            };
            for (visible_index, (index, row)) in rows
                .iter()
                .enumerate()
                .take(range.end)
                .skip(range.start)
                .enumerate()
            {
                set_styled_row(
                    &mut frame,
                    2 + visible_index,
                    tree_row_bytes(row, index == *selected, &row_context)?,
                );
            }
            if view.omitted_nodes > 0 {
                set_plain_row(
                    &mut frame,
                    2 + range.len(),
                    &format!("… {} more nodes", view.omitted_nodes),
                    width,
                );
            }
            if message.is_empty() {
                set_plain_row(
                    &mut frame,
                    height.saturating_sub(3),
                    "↑↓ Navigate   Enter Focus   Space Fold",
                    width,
                );
                set_muted_row(
                    &mut frame,
                    height.saturating_sub(1),
                    "s Settings   q Close",
                    width,
                );
            } else {
                set_plain_row(&mut frame, height.saturating_sub(1), message, width);
            }
        }
        Screen::Settings { selected } => {
            set_accent_row(&mut frame, 0, "Settings", width);

            let mut row = 2;
            for (section, range) in [("Layout", 0..2), ("Behavior", 2..5), ("Tree", 5..8)] {
                set_section_row(&mut frame, row, section, width);
                row += 1;
                for index in range {
                    let item = setting_item(settings, index);
                    set_styled_row(
                        &mut frame,
                        row,
                        setting_row_bytes(&item, width, index == *selected)?,
                    );
                    row += 1;
                }
                row += 1;
            }
            if message.is_empty() {
                set_muted_row(
                    &mut frame,
                    height.saturating_sub(3),
                    "↑↓ Navigate   ←→ Change",
                    width,
                );
                set_muted_row(
                    &mut frame,
                    height.saturating_sub(1),
                    "Esc Back   q Close",
                    width,
                );
            } else {
                set_plain_row(&mut frame, height.saturating_sub(1), message, width);
            }
        }
    }
    Ok(frame)
}

fn set_plain_row(frame: &mut [Vec<u8>], row: usize, value: &str, width: usize) {
    if let Some(target) = frame.get_mut(row) {
        *target = organizations_ui::fit_terminal_row(value, width).into_bytes();
    }
}

fn set_styled_row(frame: &mut [Vec<u8>], row: usize, value: Vec<u8>) {
    if let Some(target) = frame.get_mut(row) {
        *target = value;
    }
}

fn set_accent_row(frame: &mut [Vec<u8>], row: usize, value: &str, width: usize) {
    set_styled_row(
        frame,
        row,
        simple_styled_row(value, width, Color::Blue, Attribute::Bold),
    );
}

fn set_section_row(frame: &mut [Vec<u8>], row: usize, value: &str, width: usize) {
    set_styled_row(
        frame,
        row,
        simple_styled_row(value, width, Color::Grey, Attribute::Bold),
    );
}

fn set_muted_row(frame: &mut [Vec<u8>], row: usize, value: &str, width: usize) {
    set_styled_row(
        frame,
        row,
        simple_styled_row(value, width, Color::DarkGrey, Attribute::NormalIntensity),
    );
}

fn simple_styled_row(value: &str, width: usize, color: Color, attribute: Attribute) -> Vec<u8> {
    let mut output = Vec::new();
    execute!(
        &mut output,
        SetForegroundColor(color),
        SetAttribute(attribute)
    )
    .expect("writing to a byte buffer cannot fail");
    write!(
        &mut output,
        "{}",
        organizations_ui::fit_terminal_row(value, width)
    )
    .expect("writing to a byte buffer cannot fail");
    execute!(&mut output, ResetColor, SetAttribute(Attribute::Reset))
        .expect("writing to a byte buffer cannot fail");
    output
}

fn tree_row_bytes(
    row: &VisibleRow<'_>,
    selected: bool,
    context: &TreeRowContext<'_>,
) -> Result<Vec<u8>> {
    let mut output = Vec::new();
    write_tree_row(&mut output, row, selected, context)?;
    output.truncate(output.len().saturating_sub(2));
    Ok(output)
}

struct TreeRowContext<'a> {
    view: &'a TreeView,
    settings: &'a SidebarSettings,
    collapsed: &'a BTreeSet<String>,
    root_collapsed: bool,
    width: usize,
}

fn write_tree_row(
    writer: &mut impl Write,
    row: &VisibleRow<'_>,
    selected: bool,
    context: &TreeRowContext<'_>,
) -> Result<()> {
    let (text, color) = row_text_with_color(
        context.view,
        row,
        context.settings,
        context.collapsed,
        context.root_collapsed,
    );
    let marker_width = context.width.min(2);
    if selected {
        execute!(
            writer,
            SetForegroundColor(Color::Blue),
            SetAttribute(Attribute::Bold)
        )?;
        write!(
            writer,
            "{}",
            organizations_ui::fit_terminal_row("› ", marker_width)
        )?;
    } else {
        write!(writer, "{}", " ".repeat(marker_width))?;
    }
    let text =
        organizations_ui::fit_terminal_row(&text, context.width.saturating_sub(marker_width));
    if context.settings.show_status
        && let Some(status_start) = text.rfind("  ● ")
    {
        let role = match row {
            VisibleRow::Root => "coordinator",
            VisibleRow::Entry(entry) => entry.thread.role.as_str(),
        };
        let role_suffix = format!("  {role}");
        let role_start = context
            .settings
            .show_role
            .then(|| text.strip_suffix(&role_suffix).map(str::len))
            .flatten()
            .filter(|start| *start >= status_start);
        write!(writer, "{}", &text[..status_start])?;
        execute!(writer, SetAttribute(Attribute::Reset))?;
        if let Some(color) = color {
            execute!(writer, SetForegroundColor(color))?;
        }
        write!(
            writer,
            "{}",
            &text[status_start..role_start.unwrap_or(text.len())]
        )?;
        execute!(writer, ResetColor)?;
        if let Some(role_start) = role_start {
            execute!(writer, SetForegroundColor(Color::DarkGrey))?;
            write!(writer, "{}", &text[role_start..])?;
            execute!(writer, ResetColor)?;
        }
    } else {
        write!(writer, "{text}")?;
    }
    execute!(writer, ResetColor, SetAttribute(Attribute::Reset))?;
    write!(writer, "\r\n")?;
    Ok(())
}

fn row_text(
    view: &TreeView,
    row: &VisibleRow<'_>,
    settings: &SidebarSettings,
    collapsed: &BTreeSet<String>,
    root_collapsed: bool,
) -> String {
    row_text_with_color(view, row, settings, collapsed, root_collapsed).0
}

fn row_text_with_color(
    view: &TreeView,
    row: &VisibleRow<'_>,
    settings: &SidebarSettings,
    collapsed: &BTreeSet<String>,
    root_collapsed: bool,
) -> (String, Option<Color>) {
    match row {
        VisibleRow::Root => {
            let has_children = !view.entries.is_empty();
            let disclosure = if !has_children {
                "  "
            } else if root_collapsed {
                "▸ "
            } else {
                "▾ "
            };
            let status = view.project.status().to_string();
            let mut text = format!("{disclosure}Project coordinator");
            if settings.show_status {
                text.push_str(&format!("  ● {status}"));
            }
            if settings.show_role {
                text.push_str("  coordinator");
            }
            let color = match view.project.status() {
                Status::Active => Color::Green,
                Status::Paused => Color::Yellow,
                Status::Archived => Color::DarkGrey,
            };
            (text, settings.show_status.then_some(color))
        }
        VisibleRow::Entry(entry) => {
            let has_children = view
                .entries
                .iter()
                .any(|candidate| organizations::parent_id(&candidate.thread) == entry.thread.id);
            let disclosure = if entry.thread.role != NodeRole::Coordinator || !has_children {
                "  "
            } else if collapsed.contains(&entry.thread.id) {
                "▸ "
            } else {
                "▾ "
            };
            let group = view
                .groups
                .get(&entry.thread.id)
                .copied()
                .unwrap_or_else(|| {
                    Group::from_token(&entry.thread.last_group).unwrap_or(
                        match entry.thread.status {
                            thread_model::Status::Starting => Group::Working,
                            thread_model::Status::Open => Group::Idle,
                            thread_model::Status::Failed => Group::WaitingOnYou,
                            thread_model::Status::Resolved => Group::Resolved,
                        },
                    )
                });
            let mut text = format!("{}{disclosure}{}", entry.prefix, entry.thread.title);
            if settings.show_status {
                text.push_str(&format!("  ● {}", group.label()));
            }
            if settings.show_role {
                text.push_str(&format!("  {}", entry.thread.role.as_str()));
            }
            let color = match group {
                Group::ReadyForReview => Color::Blue,
                Group::WaitingOnYou => Color::Yellow,
                Group::Working | Group::Landing => Color::Green,
                Group::Idle | Group::Resolved => Color::DarkGrey,
            };
            (text, settings.show_status.then_some(color))
        }
    }
}

struct SettingItem {
    label: &'static str,
    value: String,
    enabled: Option<bool>,
}

fn setting_item(settings: &SidebarSettings, index: usize) -> SettingItem {
    let toggle = |label, value| SettingItem {
        label,
        value: if value { "● On" } else { "○ Off" }.into(),
        enabled: Some(value),
    };
    match index {
        0 => SettingItem {
            label: "Dock",
            value: settings.dock_side.label().into(),
            enabled: None,
        },
        1 => SettingItem {
            label: "Width",
            value: format!("{}%", settings.width_percent),
            enabled: None,
        },
        2 => toggle("Focus on open", settings.focus_on_open),
        3 => toggle("Open with project", settings.auto_open),
        4 => toggle("Close on shortcut", settings.strict_toggle),
        5 => toggle("Resolved nodes", settings.show_resolved),
        6 => toggle("Status", settings.show_status),
        7 => toggle("Roles", settings.show_role),
        _ => unreachable!("setting index is bounded by SETTINGS_COUNT"),
    }
}

fn setting_row_bytes(item: &SettingItem, width: usize, selected: bool) -> Result<Vec<u8>> {
    let mut output = Vec::new();
    let marker_width = width.min(2);
    let content_width = width.saturating_sub(marker_width);
    let value = organizations_ui::fit_terminal_row(&item.value, content_width);
    let value_width = UnicodeWidthStr::width(value.as_str());
    let value_column = SETTINGS_VALUE_COLUMN.min(width.saturating_sub(value_width));
    let label_width = value_column.saturating_sub(marker_width + 2);
    let label = organizations_ui::fit_terminal_row(item.label, label_width);
    let padding = value_column
        .saturating_sub(marker_width + UnicodeWidthStr::width(label.as_str()))
        .min(content_width);

    if selected {
        execute!(
            &mut output,
            SetForegroundColor(Color::Blue),
            SetAttribute(Attribute::Bold)
        )?;
        write!(
            &mut output,
            "{}{label}",
            organizations_ui::fit_terminal_row("› ", marker_width)
        )?;
    } else {
        write!(&mut output, "{}{label}", " ".repeat(marker_width))?;
    }
    execute!(&mut output, ResetColor, SetAttribute(Attribute::Reset))?;
    write!(&mut output, "{}", " ".repeat(padding))?;
    let value_color = match item.enabled {
        Some(true) => Color::Green,
        Some(false) => Color::Grey,
        None if selected => Color::Blue,
        None => Color::Grey,
    };
    execute!(&mut output, SetForegroundColor(value_color))?;
    write!(&mut output, "{value}")?;
    execute!(&mut output, ResetColor, SetAttribute(Attribute::Reset))?;
    Ok(output)
}

fn modify_setting(settings: &mut SidebarSettings, selected: usize, direction: isize) {
    let set_bool = |value: &mut bool| match direction {
        -1 => *value = false,
        1 => *value = true,
        _ => *value = !*value,
    };
    match selected {
        0 => {
            settings.dock_side = match direction {
                -1 => DockSide::Left,
                1 => DockSide::Right,
                _ if settings.dock_side == DockSide::Right => DockSide::Left,
                _ => DockSide::Right,
            };
        }
        1 => {
            const WIDTHS: [u8; 8] = [15, 20, 25, 30, 35, 40, 45, 50];
            let index = match direction {
                -1 => WIDTHS
                    .partition_point(|width| *width < settings.width_percent)
                    .saturating_sub(1),
                _ => WIDTHS
                    .partition_point(|width| *width <= settings.width_percent)
                    .min(WIDTHS.len() - 1),
            };
            settings.width_percent = WIDTHS[index];
        }
        2 => set_bool(&mut settings.focus_on_open),
        3 => set_bool(&mut settings.auto_open),
        4 => set_bool(&mut settings.strict_toggle),
        5 => set_bool(&mut settings.show_resolved),
        6 => set_bool(&mut settings.show_status),
        7 => set_bool(&mut settings.show_role),
        _ => {}
    }
}

fn visible_range(count: usize, selected: usize, capacity: usize) -> Range<usize> {
    if count <= capacity {
        return 0..count;
    }
    let capacity = capacity.max(1).min(count);
    let start = selected.saturating_sub(capacity - 1).min(count - capacity);
    start..start + capacity
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;

    use crate::runner::fake::{FakeRunner, fail, ok};
    use crate::scenarios::World;

    fn plugin_env(world: &World) -> crate::paths::Env {
        let config = world.home.path().join("plugins").join("herdr-projects");
        let socket = world.home.path().join("a.sock");
        let vars = [
            ("HERDR_PLUGIN_CONFIG_DIR", config.to_str().unwrap()),
            ("HERDR_SOCKET_PATH", socket.to_str().unwrap()),
        ];
        crate::paths::Env::for_test(world.home.path(), &vars)
    }

    fn plugin_ctx<'a>(world: &'a World, env: &'a crate::paths::Env) -> Ctx<'a> {
        Ctx {
            env,
            root: world.root.clone(),
            config_dir: world.home.path().join("cfg"),
            runner: &world.runner,
            detached_ticker: false,
        }
    }

    fn identity_tokens(slug: &str, workspace: &str) -> serde_json::Value {
        let mut tokens = serde_json::Map::new();
        tokens.insert(TOKEN_ID.into(), serde_json::json!("v1"));
        tokens.insert(TOKEN_PROJECT.into(), serde_json::json!(slug));
        tokens.insert(TOKEN_WORKSPACE.into(), serde_json::json!(workspace));
        serde_json::Value::Object(tokens)
    }

    #[test]
    fn a_new_split_retries_metadata_until_herdr_registers_the_pane() {
        let runner = FakeRunner::new();
        let attempts = Rc::new(Cell::new(0));
        let recorded_attempts = Rc::clone(&attempts);
        runner.on_fn(
            |command| command.display().contains("report-metadata"),
            move |_| {
                let attempt = recorded_attempts.get() + 1;
                recorded_attempts.set(attempt);
                Ok(if attempt < 3 {
                    fail(1, "pane not registered yet")
                } else {
                    ok(r#"{"result":{}}"#)
                })
            },
        );
        let herdr = Herdr::new("herdr", "/tmp/herdr.sock", &runner);

        report_identity_after_split(&herdr, "w1:p2", "demo", "w1").unwrap();

        assert_eq!(attempts.get(), 3);
    }

    #[test]
    fn settings_are_saved_under_the_herdr_plugin_config_directory() {
        let world = World::new();
        let config = world.home.path().join("plugins").join("herdr-projects");
        let env = crate::paths::Env::for_test(
            world.home.path(),
            &[("HERDR_PLUGIN_CONFIG_DIR", config.to_str().unwrap())],
        );
        let ctx = Ctx {
            env: &env,
            ..world.ctx()
        };
        let settings = SidebarSettings {
            dock_side: DockSide::Left,
            width_percent: 42,
            auto_open: true,
            ..SidebarSettings::default()
        };

        save_settings(&ctx, &settings).unwrap();

        let loaded = load_settings(&ctx).unwrap();
        assert_eq!(loaded.dock_side, DockSide::Left);
        assert_eq!(loaded.width_percent, 42);
        assert!(loaded.auto_open);
        assert!(config.join(CONFIG_FILE).is_file());
    }

    #[test]
    fn defaults_keep_the_sidebar_right_compact_and_non_intrusive() {
        let settings = SidebarSettings::default();
        assert_eq!(settings.dock_side, DockSide::Right);
        assert_eq!(settings.width_percent, 30);
        assert!(!settings.focus_on_open);
        assert!(!settings.auto_open);
        assert!(settings.show_status);
        assert!(settings.show_role);
        assert!(settings.strict_toggle);
    }

    #[test]
    fn unchanged_periodic_refreshes_do_not_dirty_the_terminal_view() {
        let world = World::new();
        let project = world.project("demo", "a.sock");
        let current = TreeView {
            project,
            entries: organizations::tree_from(&[thread_model::Thread {
                id: "t-0001".into(),
                title: "Stable worker".into(),
                ..thread_model::Thread::default()
            }])
            .unwrap(),
            groups: HashMap::from([("t-0001".into(), Group::Working)]),
            omitted_nodes: 0,
        };

        assert!(!tree_view_changed(&current, &current.clone()));

        let mut changed = current.clone();
        changed
            .groups
            .insert("t-0001".into(), Group::ReadyForReview);
        assert!(tree_view_changed(&current, &changed));
    }

    #[test]
    fn moving_selection_repaints_only_the_two_changed_rows() {
        let world = World::new();
        let project = world.project("demo", "a.sock");
        let view = TreeView {
            project,
            entries: organizations::tree_from(&[thread_model::Thread {
                id: "t-0001".into(),
                title: "Worker".into(),
                ..thread_model::Thread::default()
            }])
            .unwrap(),
            groups: HashMap::from([("t-0001".into(), Group::Working)]),
            omitted_nodes: 0,
        };
        let settings = SidebarSettings::default();
        let collapsed = BTreeSet::new();
        let mut previous = Vec::new();
        let mut output = Vec::new();
        let initial = Screen::Tree {
            view: view.clone(),
            collapsed: collapsed.clone(),
            root_collapsed: false,
            selected: 0,
        };
        render_incremental(&mut output, &mut previous, &initial, &settings, "", 80, 24).unwrap();
        let full_render_bytes = output.len();

        output.clear();
        let moved = Screen::Tree {
            view,
            collapsed,
            root_collapsed: false,
            selected: 1,
        };
        render_incremental(&mut output, &mut previous, &moved, &settings, "", 80, 24).unwrap();
        let mut clear_sequence = Vec::new();
        execute!(&mut clear_sequence, Clear(ClearType::CurrentLine)).unwrap();
        let changed_rows = output
            .windows(clear_sequence.len())
            .filter(|window| *window == clear_sequence)
            .count();
        assert_eq!(changed_rows, 2);
        assert!(output.len() < full_render_bytes);

        output.clear();
        render_incremental(&mut output, &mut previous, &moved, &settings, "", 80, 24).unwrap();
        assert!(output.is_empty());
    }

    #[test]
    fn nested_coordinators_collapse_their_descendants_only() {
        let world = World::new();
        let project = world.project("demo", "a.sock");
        let records = [
            thread_model::Thread {
                id: "t-0001".into(),
                title: "Frontend".into(),
                role: NodeRole::Coordinator,
                can_spawn: true,
                ..thread_model::Thread::default()
            },
            thread_model::Thread {
                id: "t-0002".into(),
                parent_id: "t-0001".into(),
                title: "Design".into(),
                ..thread_model::Thread::default()
            },
            thread_model::Thread {
                id: "t-0003".into(),
                title: "Backend".into(),
                ..thread_model::Thread::default()
            },
        ];
        let entries = organizations::tree_from(&records).unwrap();
        let view = TreeView {
            project,
            entries,
            groups: HashMap::new(),
            omitted_nodes: 0,
        };
        let mut collapsed = BTreeSet::from(["t-0001".to_string()]);
        let rows = visible_rows(&view, &collapsed, false);
        assert_eq!(rows.len(), 3);
        assert!(matches!(rows[1], VisibleRow::Entry(entry) if entry.thread.id == "t-0001"));
        assert!(matches!(rows[2], VisibleRow::Entry(entry) if entry.thread.id == "t-0003"));
        collapsed.clear();
        assert_eq!(visible_rows(&view, &collapsed, true).len(), 1);
    }

    #[test]
    fn rows_lead_with_titles_color_status_and_can_hide_status_and_role() {
        let world = World::new();
        let project = world.project("demo", "a.sock");
        let entries = organizations::tree_from(&[thread_model::Thread {
            id: "t-0001".into(),
            title: "Review the release".into(),
            ..thread_model::Thread::default()
        }])
        .unwrap();
        let view = TreeView {
            project,
            entries,
            groups: HashMap::from([("t-0001".into(), Group::ReadyForReview)]),
            omitted_nodes: 0,
        };
        let row = VisibleRow::Entry(&view.entries[0]);
        let default_settings = SidebarSettings::default();
        let (visible_text, visible_color) =
            row_text_with_color(&view, &row, &default_settings, &BTreeSet::new(), false);

        let title_position = visible_text.find("Review the release").unwrap();
        let status_position = visible_text.find("Ready for review").unwrap();
        assert!(title_position < status_position);
        assert!(visible_text.contains("worker"));
        assert_eq!(visible_color, Some(Color::Blue));

        let hidden_settings = SidebarSettings {
            show_status: false,
            show_role: false,
            ..SidebarSettings::default()
        };
        let (hidden_text, hidden_color) =
            row_text_with_color(&view, &row, &hidden_settings, &BTreeSet::new(), false);
        assert!(hidden_text.contains("Review the release"));
        assert!(!hidden_text.contains("Ready for review"));
        assert!(!hidden_text.contains("worker"));
        assert_eq!(hidden_color, None);
    }

    #[test]
    fn selected_tree_rows_use_a_quiet_marker_and_reset_style_before_crlf() {
        let world = World::new();
        let project = world.project("demo", "a.sock");
        let entries = organizations::tree_from(&[thread_model::Thread {
            id: "t-0001".into(),
            title: "Review the release".into(),
            ..thread_model::Thread::default()
        }])
        .unwrap();
        let view = TreeView {
            project,
            entries,
            groups: HashMap::from([("t-0001".into(), Group::ReadyForReview)]),
            omitted_nodes: 0,
        };
        let row = VisibleRow::Entry(&view.entries[0]);
        let mut output = Vec::new();

        let settings = SidebarSettings::default();
        let collapsed = BTreeSet::new();
        let context = TreeRowContext {
            view: &view,
            settings: &settings,
            collapsed: &collapsed,
            root_collapsed: false,
            width: 120,
        };
        write_tree_row(&mut output, &row, true, &context).unwrap();

        let rendered = str::from_utf8(&output).unwrap();
        let mut blue_sequence = Vec::new();
        execute!(&mut blue_sequence, SetForegroundColor(Color::Blue)).unwrap();
        let blue_sequence = String::from_utf8(blue_sequence).unwrap();
        let title = rendered.find("Review the release").unwrap();
        let blue = rendered.find(&blue_sequence).unwrap();
        assert!(blue < title, "the selection marker should lead the title");
        assert!(rendered.contains("› "));
        let mut reverse_sequence = Vec::new();
        execute!(&mut reverse_sequence, SetAttribute(Attribute::Reverse)).unwrap();
        assert!(
            !output
                .windows(reverse_sequence.len())
                .any(|window| window == reverse_sequence)
        );
        let mut reset_suffix = Vec::new();
        execute!(
            &mut reset_suffix,
            ResetColor,
            SetAttribute(Attribute::Reset)
        )
        .unwrap();
        reset_suffix.extend_from_slice(b"\r\n");
        assert!(output.ends_with(&reset_suffix));
    }

    #[test]
    fn settings_are_grouped_and_hide_implementation_details() {
        let settings = SidebarSettings::default();
        let mut output = Vec::new();
        render(
            &mut output,
            &Screen::Settings { selected: 0 },
            &settings,
            "",
            120,
            24,
        )
        .unwrap();
        let output = String::from_utf8(output).unwrap();
        assert!(output.contains("Settings"));
        assert!(output.contains("Layout"));
        assert!(output.contains("Behavior"));
        assert!(output.contains("Tree"));
        assert!(output.contains("› Dock"));
        assert!(!output.contains("Sidebar settings"));
        assert!(!output.contains("Changes save automatically"));
        assert!(!output.contains("organization-sidebar"));
        assert!(!output.contains("Strict toggle"));
    }

    #[test]
    fn setting_values_share_a_compact_column_instead_of_the_pane_edge() {
        let settings = SidebarSettings::default();
        let dock =
            String::from_utf8(setting_row_bytes(&setting_item(&settings, 0), 80, false).unwrap())
                .unwrap();
        let width =
            String::from_utf8(setting_row_bytes(&setting_item(&settings, 1), 80, false).unwrap())
                .unwrap();
        let dock_value = dock.find("Right").unwrap();
        let width_value = width.find("30%").unwrap();

        assert_eq!(dock_value, width_value);
        assert!(dock_value < 40, "values should stay near their labels");
    }

    #[test]
    fn sidebar_identity_requires_its_token_and_exact_project_workspace_pair() {
        let mut pane = Pane {
            pane_id: "w1:p2".into(),
            workspace_id: "w1".into(),
            ..Pane::default()
        };
        pane.tokens
            .insert(TOKEN_ID.into(), serde_json::Value::String("v1".into()));
        pane.tokens.insert(
            TOKEN_PROJECT.into(),
            serde_json::Value::String("demo".into()),
        );
        pane.tokens.insert(
            TOKEN_WORKSPACE.into(),
            serde_json::Value::String("w1".into()),
        );
        assert!(sidebar_pane(&pane, "demo", "w1"));
        assert!(!sidebar_pane(&pane, "other", "w1"));
        assert!(!sidebar_pane(&pane, "demo", "w2"));
        pane.tokens.remove(TOKEN_ID);
        assert!(!sidebar_pane(&pane, "demo", "w1"));
    }

    #[test]
    fn width_and_toggle_settings_are_changed_without_touching_global_keybindings() {
        let mut settings = SidebarSettings::default();
        modify_setting(&mut settings, 1, 1);
        modify_setting(&mut settings, 2, 0);
        modify_setting(&mut settings, 5, 0);
        assert_eq!(settings.width_percent, 35);
        assert!(settings.focus_on_open);
        assert!(settings.show_resolved);

        settings.width_percent = 16;
        modify_setting(&mut settings, 1, -1);
        assert_eq!(settings.width_percent, 15);
        modify_setting(&mut settings, 1, 1);
        assert_eq!(settings.width_percent, 20);
    }

    #[test]
    fn strict_toggle_closes_only_the_matching_sidebar_pane() {
        let world = World::new();
        let project = world.project("demo", "a.sock");
        let mut own: serde_json::Value =
            serde_json::from_str(&world.coordinator_pane(&project)).unwrap();
        own["pane_id"] = serde_json::json!("w1:p-sidebar");
        own["focused"] = serde_json::json!(false);
        own["tokens"] = identity_tokens("demo", "w1");
        let mut unrelated: serde_json::Value =
            serde_json::from_str(&world.coordinator_pane(&project)).unwrap();
        unrelated["pane_id"] = serde_json::json!("w1:p-other");
        unrelated["label"] = serde_json::json!("Org Hierarchy");
        *world.panes.borrow_mut() = serde_json::json!([own, unrelated]).to_string();
        world.runner.on("pane close", ok(r#"{"result":{}}"#));
        let env = plugin_env(&world);
        let ctx = plugin_ctx(&world, &env);

        let result = toggle(&ctx, "demo", "w1", "w1:p1").unwrap();

        assert_eq!(result, ToggleResult::Closed);
        let calls = world.runner.calls.borrow();
        let closed = calls
            .iter()
            .find(|call| call.args.starts_with(&["pane".into(), "close".into()]))
            .unwrap();
        assert_eq!(closed.args, ["pane", "close", "w1:p-sidebar"]);
        assert_eq!(
            calls
                .iter()
                .filter(|call| call.args.starts_with(&["pane".into(), "close".into()]))
                .count(),
            1
        );
    }

    #[test]
    fn non_strict_toggle_focuses_an_existing_owned_sidebar_without_splitting() {
        let world = World::new();
        let project = world.project("demo", "a.sock");
        let mut own: serde_json::Value =
            serde_json::from_str(&world.coordinator_pane(&project)).unwrap();
        own["pane_id"] = serde_json::json!("w1:p-sidebar");
        own["focused"] = serde_json::json!(false);
        own["tokens"] = identity_tokens("demo", "w1");
        *world.panes.borrow_mut() = serde_json::json!([own]).to_string();
        world.runner.on("plugin pane focus", ok(r#"{"result":{}}"#));
        let env = plugin_env(&world);
        let ctx = plugin_ctx(&world, &env);
        save_settings(
            &ctx,
            &SidebarSettings {
                strict_toggle: false,
                focus_on_open: true,
                ..SidebarSettings::default()
            },
        )
        .unwrap();

        assert_eq!(
            toggle(&ctx, "demo", "w1", "w1:p1").unwrap(),
            ToggleResult::Focused
        );
        assert_eq!(world.runner.count("plugin pane focus"), 1);
        assert_eq!(world.runner.count("pane close"), 0);
        assert_eq!(world.runner.count("pane split"), 0);
    }

    #[test]
    fn toggle_opens_a_right_split_and_stamps_project_workspace_identity() {
        let world = World::new();
        let project = world.project("demo", "a.sock");
        let source: serde_json::Value =
            serde_json::from_str(&world.coordinator_pane(&project)).unwrap();
        let source_cwd = source["cwd"].as_str().unwrap().to_string();
        *world.panes.borrow_mut() = serde_json::json!([source]).to_string();
        world.runner.on(
            "pane split",
            ok(r#"{"result":{"pane":{"pane_id":"w1:p2","tab_id":"w1:t1","workspace_id":"w1","cwd":"/project"}}}"#),
        );
        world.runner.on("pane rename", ok(r#"{"result":{}}"#));
        world.runner.on("report-metadata", ok(r#"{"result":{}}"#));
        world.runner.on("pane run", ok(r#"{"result":{}}"#));
        let env = plugin_env(&world);
        let ctx = plugin_ctx(&world, &env);

        let result = toggle(&ctx, "demo", "w1", "w1:p1").unwrap();

        assert_eq!(result, ToggleResult::Opened);
        let calls = world.runner.calls.borrow();
        let split = calls
            .iter()
            .find(|call| call.args.starts_with(&["pane".into(), "split".into()]))
            .unwrap();
        assert_eq!(
            &split.args[..8],
            [
                "pane",
                "split",
                "w1:p1",
                "--direction",
                "right",
                "--ratio",
                "0.7",
                "--no-focus"
            ]
        );
        assert_eq!(split.args[8], "--cwd");
        assert_eq!(split.args[9], source_cwd);
        assert!(split.display().contains("--ratio 0.7"));
        assert!(split.display().contains("--no-focus"));
        let metadata = calls
            .iter()
            .find(|call| {
                call.args
                    .starts_with(&["pane".into(), "report-metadata".into()])
            })
            .unwrap();
        assert!(metadata.display().contains("--source herdr-projects"));
        assert!(metadata.display().contains("org_sidebar=v1"));
        assert!(metadata.display().contains("org_project=demo"));
        assert!(metadata.display().contains("org_workspace=w1"));
        let run = calls
            .iter()
            .find(|call| call.args.starts_with(&["pane".into(), "run".into()]))
            .unwrap();
        assert_eq!(&run.args[..4], ["pane", "run", "w1:p2", "env"]);
        assert!(run.args.iter().any(|arg| arg == "HERDR_PANE_ID=w1:p2"));
        assert!(
            run.args
                .iter()
                .any(|arg| arg == "HERDR_ORGANIZATIONS_PROJECT=demo")
        );
        assert_eq!(
            &run.args[run.args.len() - 2..],
            ["pane", "organization-sidebar"]
        );
    }

    #[test]
    fn configured_left_dock_uses_the_width_and_focus_settings() {
        let world = World::new();
        let project = world.project("demo", "a.sock");
        *world.panes.borrow_mut() = format!("[{}]", world.coordinator_pane(&project));
        world.runner.on(
            "pane split",
            ok(r#"{"result":{"pane":{"pane_id":"w1:p2","tab_id":"w1:t1","workspace_id":"w1","cwd":"/project"}}}"#),
        );
        world.runner.on("pane swap", ok(r#"{"result":{}}"#));
        world.runner.on("pane rename", ok(r#"{"result":{}}"#));
        world.runner.on("report-metadata", ok(r#"{"result":{}}"#));
        world.runner.on("pane run", ok(r#"{"result":{}}"#));
        world.runner.on("pane focus", ok(r#"{"result":{}}"#));
        let env = plugin_env(&world);
        let ctx = plugin_ctx(&world, &env);
        save_settings(
            &ctx,
            &SidebarSettings {
                dock_side: DockSide::Left,
                width_percent: 40,
                focus_on_open: true,
                ..SidebarSettings::default()
            },
        )
        .unwrap();

        assert_eq!(
            toggle(&ctx, "demo", "w1", "w1:p1").unwrap(),
            ToggleResult::Opened
        );
        let calls = world.runner.calls.borrow();
        let split = calls
            .iter()
            .find(|call| call.args.starts_with(&["pane".into(), "split".into()]))
            .unwrap();
        assert!(split.display().contains("--ratio 0.4"));
        assert!(calls.iter().any(|call| call.args
            == [
                "pane",
                "swap",
                "--source-pane",
                "w1:p2",
                "--target-pane",
                "w1:p1"
            ]));
        assert!(
            calls.iter().any(
                |call| call.args == ["pane", "focus", "--direction", "left", "--pane", "w1:p1"]
            )
        );
    }

    #[test]
    fn auto_open_is_opt_in_and_ensure_never_toggles_an_existing_sidebar_closed() {
        let world = World::new();
        let project = world.project("demo", "a.sock");
        let env = plugin_env(&world);
        let ctx = plugin_ctx(&world, &env);
        assert_eq!(
            ensure_auto_open(&ctx, Some("demo"), "w1", "w1:p1").unwrap(),
            ToggleResult::AlreadyOpen
        );
        assert_eq!(world.runner.calls.borrow().len(), 0);

        let settings = SidebarSettings {
            auto_open: true,
            ..SidebarSettings::default()
        };
        save_settings(&ctx, &settings).unwrap();
        let mut own: serde_json::Value =
            serde_json::from_str(&world.coordinator_pane(&project)).unwrap();
        own["pane_id"] = serde_json::json!("w1:p-sidebar");
        own["tokens"] = identity_tokens("demo", "w1");
        *world.panes.borrow_mut() = serde_json::json!([own]).to_string();

        assert_eq!(
            ensure_auto_open(&ctx, Some("demo"), "w1", "w1:p1").unwrap(),
            ToggleResult::AlreadyOpen
        );
        assert_eq!(world.runner.count("pane close"), 0);
        assert_eq!(world.runner.count("pane split"), 0);
    }

    #[test]
    fn enabled_auto_open_creates_a_sidebar_once() {
        let world = World::new();
        let project = world.project("demo", "a.sock");
        *world.panes.borrow_mut() = format!("[{}]", world.coordinator_pane(&project));
        world.runner.on(
            "pane split",
            ok(r#"{"result":{"pane":{"pane_id":"w1:p2","tab_id":"w1:t1","workspace_id":"w1","cwd":"/project"}}}"#),
        );
        world.runner.on("pane rename", ok(r#"{"result":{}}"#));
        world.runner.on("report-metadata", ok(r#"{"result":{}}"#));
        world.runner.on("pane run", ok(r#"{"result":{}}"#));
        let env = plugin_env(&world);
        let ctx = plugin_ctx(&world, &env);
        save_settings(
            &ctx,
            &SidebarSettings {
                auto_open: true,
                ..SidebarSettings::default()
            },
        )
        .unwrap();

        assert_eq!(
            ensure_auto_open(&ctx, Some("demo"), "w1", "w1:p1").unwrap(),
            ToggleResult::Opened
        );
        assert_eq!(world.runner.count("pane split"), 1);
        assert_eq!(world.runner.count("pane run"), 1);
    }
}

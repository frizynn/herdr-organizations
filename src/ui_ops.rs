//! What the popup and the dock do when a key asks for it. Each is one user
//! action: these are the only places the views call Herdr, git or gh, and
//! every one reuses the CLI mechanic of the same name.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::coordinator;
use crate::herdr::{CALL_TIMEOUT, Herdr};
use crate::organizations::{NodeRequest, ROOT_ID};
use crate::paths::{Ctx, SessionFlags};
use crate::project::{self, Project};
use crate::state::{self, Live, Snapshot};
use crate::thread::{self, Kind, NodeRole, Thread};
use crate::{agent_profile, inbox, pr, steps, term, threads};

/// The socket this view was opened from.
pub fn env_socket(ctx: &Ctx) -> Option<PathBuf> {
    ctx.env
        .var("HERDR_SOCKET_PATH")
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
}

/// The state file when a ticker keeps it fresh, else the same model built
/// now from the records plus four cheap socket reads.
pub fn load_snapshot(ctx: &Ctx) -> Snapshot {
    let ticker_running = crate::ticker::lock_state(&ctx.root) != crate::ticker::LockState::Free;
    if ticker_running && let Some(snapshot) = state::read(&ctx.root) {
        return snapshot;
    }
    let mut lives = Vec::new();
    if let Some(socket) = env_socket(ctx) {
        let herdr = Herdr::new(ctx.env.herdr_bin(), &socket, ctx.runner);
        if let (Ok(agents), Ok(panes)) = (herdr.agent_list_rpc(), herdr.pane_list_rpc()) {
            lives.push(Live {
                socket: socket.to_string_lossy().into_owned(),
                agents,
                panes,
                workspaces: herdr.workspace_list_rpc().unwrap_or_default(),
                tabs: herdr.tab_list_rpc().unwrap_or_default(),
            });
        }
    }
    state::build(ctx, &lives)
}

fn project_herdr<'a>(ctx: &'a Ctx, project: &Project) -> Result<Herdr<'a>> {
    let record = project
        .coordinator()
        .context("the project has not been opened yet")?;
    Ok(Herdr::new(ctx.env.herdr_bin(), record.socket, ctx.runner))
}

/// A move to a node or a coordinator that may open the coordinator or restart
/// the node: it can move focus and then wait for an agent to start (up to
/// `AGENT_START_TIMEOUT`). Herdr keeps a popup modal until its process exits,
/// so the popup hands a jump to a process of its own and closes instead of
/// waiting: a waiting popup takes every key, even after focus has moved away.
#[derive(Debug, Clone, PartialEq, clap::Subcommand)]
pub enum Jump {
    /// Focus a node (`root`: the coordinator), opening or restarting it first
    Node { slug: String, id: String },
    /// Open the project's coordinator, as `open` does
    Coordinator { slug: String },
    /// Stop the project's coordinator and start one on `profile`
    Replace { slug: String, profile: String },
    /// Open a file in a new tab, as `open-file` does (a coordinator's STATUS.md)
    File {
        path: String,
        #[arg(long)]
        workspace: Option<String>,
    },
}

impl Jump {
    /// The hidden CLI command that makes this jump.
    pub fn args(&self) -> Vec<&str> {
        match self {
            Jump::Node { slug, id } => vec!["jump", "node", slug, id],
            Jump::Coordinator { slug } => vec!["jump", "coordinator", slug],
            Jump::Replace { slug, profile } => vec!["jump", "replace", slug, profile],
            Jump::File { path, workspace } => {
                let mut args = vec!["jump", "file", path.as_str()];
                if let Some(workspace) = workspace {
                    args.extend(["--workspace", workspace.as_str()]);
                }
                args
            }
        }
    }

    pub fn run(&self, ctx: &Ctx) -> Result<()> {
        match self {
            Jump::Node { slug, id } => go_to_pane(ctx, slug, id).map(|_| ()),
            Jump::Coordinator { slug } => {
                term::quiet(|| crate::actions::run_on_slug(ctx, "open", slug))
            }
            Jump::Replace { slug, profile } => {
                replace_coordinator(ctx, slug, profile, &Default::default()).map(|_| ())
            }
            Jump::File { path, workspace } => term::quiet(|| {
                crate::settings::open_file(ctx, Path::new(path), workspace.as_deref())
            }),
        }
    }

    /// Starts [`Jump::args`] in its own session, so Herdr
    /// closing the popup does not end it. It waits on a pipe whose write end
    /// this process holds until it exits: focus moves after the popup closed.
    pub fn hand_off(&self, ctx: &Ctx) -> Result<()> {
        let (popup_exited, popup_alive) = std::io::pipe().context("could not hand the jump off")?;
        let mut command = std::process::Command::new(crate::paths::binary()?);
        command
            .arg("--root")
            .arg(&ctx.root)
            .args(self.args())
            .stdin(popup_exited)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        crate::runner::new_session(&mut command);
        command.spawn().context("could not hand the jump off")?;
        std::mem::forget(popup_alive);
        Ok(())
    }

    /// The process `hand_off` started: once the popup's process has exited,
    /// jumps, and shows a failure as a Herdr notification, since the popup
    /// that would have shown it is gone.
    pub fn after_popup(&self, ctx: &Ctx) {
        let _ = std::io::copy(&mut std::io::stdin(), &mut std::io::sink());
        if let Err(error) = self.run(ctx)
            && let Some(socket) = env_socket(ctx)
        {
            let _ = Herdr::new(ctx.env.herdr_bin(), socket, ctx.runner)
                .notification_show("Herdr Organizations", &format!("{error:#}"));
        }
    }
}

/// Focuses a node's live agent, its tab when no agent runs, or reopens it
/// when its tab is gone. `root` is the project coordinator.
pub fn go_to_pane(ctx: &Ctx, slug: &str, id: &str) -> Result<String> {
    let project = Project::load(&ctx.root, slug)?;
    let recorded_socket = project
        .coordinator()
        .and_then(|record| (!record.socket.is_empty()).then(|| PathBuf::from(record.socket)));
    let options = coordinator::OpenOptions {
        session: SessionFlags {
            session: None,
            socket: recorded_socket
                .filter(|socket| socket.exists())
                .or_else(|| env_socket(ctx)),
        },
        rebind: true,
        profile: None,
        launch: Default::default(),
        new: false,
        here: false,
    };
    coordinator::open_quiet(ctx, slug, &options)
        .context("could not open the project coordinator")?;
    let project = Project::load(&ctx.root, slug)?;
    let record = project
        .coordinator()
        .context("the project coordinator has not been placed")?;
    let view = threads::session_view(ctx, &project)
        .context("the project's Herdr session did not become reachable")?;
    let focus = |herdr: &Herdr, pane: &str| {
        herdr
            .agent_focus(pane)
            .map(|_| pane.to_string())
            .map_err(|e| anyhow::anyhow!("{e}"))
    };
    if id == ROOT_ID {
        if view
            .agents
            .iter()
            .any(|a| a.pane_id == record.pane_id && coordinator::is_coordinator(&record, a))
        {
            return focus(&view.herdr, &record.pane_id);
        }
        if view
            .panes
            .iter()
            .any(|p| coordinator::pane_matches(&record, p))
        {
            view.herdr
                .tab_focus(&record.tab_id)
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            return Ok(record.pane_id);
        }
        bail!("the project coordinator has no live pane after opening");
    }
    let node = thread::load(&project, id)?;
    if node.status == thread::Status::Resolved {
        bail!("{id} is resolved and cannot be reopened from here");
    }
    let herdr = view.herdr.on_machine(&node.machine);
    let (agents, panes) = if node.is_remote() {
        (
            herdr.agent_list().map_err(|e| anyhow::anyhow!("{e}"))?,
            herdr.pane_list().map_err(|e| anyhow::anyhow!("{e}"))?,
        )
    } else {
        (view.agents, view.panes)
    };
    if agents.iter().any(|a| thread::agent_matches(&node, a)) {
        return focus(&herdr, &node.pane_id);
    }
    if panes.iter().any(|p| thread::pane_matches(&node, p)) {
        herdr
            .tab_focus(&node.tab_id)
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        return Ok(node.pane_id);
    }
    let node = term::quiet(|| threads::restart(ctx, slug, id, None))
        .with_context(|| format!("could not reopen {id}"))?;
    view.herdr
        .on_machine(&node.machine)
        .tab_focus(&node.tab_id)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(node.pane_id)
}

fn node_pane(project: &Project, id: &str) -> Result<(String, String)> {
    if id == ROOT_ID {
        let record = project.coordinator().context("no coordinator yet")?;
        return Ok((record.pane_id, String::new()));
    }
    let node = thread::load(project, id)?;
    Ok((node.pane_id, node.machine))
}

/// Presses one digit in a node's pane: picks an option of the dialog the
/// agent is waiting on. The user presses the key; nothing is decided here.
pub fn answer(ctx: &Ctx, slug: &str, id: &str, digit: char) -> Result<()> {
    let project = Project::load(&ctx.root, slug)?;
    let (pane, machine) = node_pane(&project, id)?;
    let herdr = project_herdr(ctx, &project)?;
    let key = digit.to_string();
    herdr
        .on_machine(&machine)
        .call(&["pane", "send-keys", &pane, &key], CALL_TIMEOUT)
        .map(|_| ())
        .map_err(|e| anyhow::anyhow!("{e}"))
}

pub fn reply(ctx: &Ctx, slug: &str, id: &str, text: &str) -> Result<()> {
    if id == ROOT_ID {
        let project = Project::load(&ctx.root, slug)?;
        let record = project.coordinator().context("no coordinator yet")?;
        return project_herdr(ctx, &project)?
            .agent_prompt(&record.pane_id, text.trim())
            .map_err(|e| anyhow::anyhow!("{e}"));
    }
    threads::prompt(ctx, slug, id, text).map(|_| ())
}

/// The last non-empty visible lines of a node's pane, read once.
pub fn last_output(ctx: &Ctx, slug: &str, id: &str, lines: usize) -> Vec<String> {
    let read = || -> Result<Vec<String>> {
        let project = Project::load(&ctx.root, slug)?;
        let (pane, machine) = node_pane(&project, id)?;
        if !machine.is_empty() || pane.is_empty() {
            return Ok(Vec::new());
        }
        let text = project_herdr(ctx, &project)?
            .pane_read_visible(&pane)
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        let mut kept: Vec<String> = text
            .lines()
            .map(str::trim_end)
            .filter(|l| !l.trim().is_empty())
            .map(str::to_string)
            .collect();
        let start = kept.len().saturating_sub(lines);
        Ok(kept.split_off(start))
    };
    read().unwrap_or_default()
}

pub fn open_pr(ctx: &Ctx, url: &str) -> Result<()> {
    if !pr::valid_pr_url(url) {
        bail!("not a pull request URL");
    }
    term::quiet(|| crate::settings::system_open(ctx, url))
}

pub fn inbox_done(ctx: &Ctx, slug: &str, item: &str) -> Result<()> {
    let project = Project::load(&ctx.root, slug)?;
    inbox::done(&project, &[item.to_string()], false).map(|_| ())
}

/// Merges with the project's `merge_method` behind the same guard as `thread
/// merge`, tells the coordinator through the inbox, then resolves the thread:
/// its worktree is removed unless `keep_worktree`, otherwise only its Herdr
/// view closes.
pub fn merge(ctx: &Ctx, slug: &str, id: &str, keep_worktree: bool) -> Result<String> {
    let project = Project::load(&ctx.root, slug)?;
    let record = thread::load(&project, id)?;
    threads::refuse_agent_pane(ctx, &project, "merging")?;
    let method = project.safety(&ctx.config_dir)?.merge_method;
    steps::merge_pull_request(ctx, &project, id, method)?;
    let _ = inbox::write(
        &project,
        "pr",
        id,
        "merged",
        &format!(
            "{id} \"{}\": pull request {} was merged from the Organizations popup",
            record.title, record.pr
        ),
        "",
    );
    let remove =
        !keep_worktree && record.kind == Kind::Worktree && !record.worktree_path.is_empty();
    let resolved = term::quiet(|| {
        threads::resolve(
            ctx,
            slug,
            id,
            &threads::ResolveArgs {
                keep_worktree: !remove,
                close_view: !remove,
                ..threads::ResolveArgs::default()
            },
        )
    });
    match resolved {
        Ok(()) => Ok(format!("merged {} and resolved {id}", record.pr)),
        Err(error) => Ok(format!("merged {}; resolve failed: {error:#}", record.pr)),
    }
}

/// What the New form creates.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct NewProject {
    pub name: String,
    pub goal: String,
    pub repos: Vec<String>,
    pub coordinator_agent: String,
    pub thread_agent: String,
    /// Set when the form was opened to continue a workspace's agent pane.
    pub adopt_pane: String,
    pub adopt_cwd: String,
}

/// Creates the project, or adopts the workspace's agent pane as one. A new
/// project's coordinator is the caller's `Jump::Coordinator`.
pub fn create_project(ctx: &Ctx, form: &NewProject) -> Result<String> {
    let socket = env_socket(ctx).context("HERDR_SOCKET_PATH is not set: open this from Herdr")?;
    if !form.adopt_pane.is_empty() {
        term::quiet(|| {
            crate::adopt::adopt_workspace(
                ctx,
                &crate::adopt::AdoptWorkspace {
                    name: form.name.clone(),
                    goal: form.goal.clone(),
                    pane: form.adopt_pane.clone(),
                    workspace_cwd: form.adopt_cwd.clone(),
                    session: SessionFlags {
                        session: None,
                        socket: Some(socket),
                    },
                },
            )
        })?;
        return project::slug_from_name(&form.name);
    }
    let repos = form
        .repos
        .iter()
        .filter(|r| !r.trim().is_empty())
        .map(|r| project::parse_repo_arg(r.trim()))
        .collect();
    let project = project::create_for(&ctx.config_dir, &ctx.root, &form.name, &form.goal, repos)?;
    // A Herdr kind is also the built-in profile of that name.
    crate::profiles::write_project_defaults(&project, &form.thread_agent, &form.coordinator_agent)?;
    Ok(project.slug)
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct NewNode {
    pub slug: String,
    pub parent: String,
    pub role: NodeRole,
    pub title: String,
    pub task: String,
    pub repo: String,
    pub harness: String,
    pub model: String,
    pub effort: String,
    /// A template of the node's role; empty for none.
    pub template: String,
}

pub fn create_node(ctx: &Ctx, form: &NewNode) -> Result<Thread> {
    let blank = |s: &str| (!s.trim().is_empty()).then(|| s.trim().to_string());
    let task = if form.task.trim().is_empty() {
        form.title.clone()
    } else {
        form.task.clone()
    };
    let mut start = threads::StartArgs {
        title: form.title.trim().to_string(),
        repo: blank(&form.repo),
        machine: None,
        profile: None,
        kind: None,
        base: None,
        task,
        node: NodeRequest {
            parent_id: form.parent.clone(),
            role: form.role,
            can_spawn: None,
            profile: agent_profile::ProfileOverrides {
                harness: blank(&form.harness),
                model: blank(&form.model),
                reasoning_effort: blank(&form.effort),
                permission_profile: None,
                raw_agent_args: Vec::new(),
            },
            ..NodeRequest::default()
        },
    };
    if let Some(name) = blank(&form.template) {
        crate::templates::resolve(ctx, Some(&form.slug), &name)?
            .fill(&mut start, Some(form.role))?;
    }
    term::quiet(|| threads::start(ctx, &form.slug, start))
}

pub fn create_workspace(ctx: &Ctx, folder: &str, label: &str) -> Result<()> {
    let socket = env_socket(ctx).context("HERDR_SOCKET_PATH is not set: open this from Herdr")?;
    let folder = ctx.env.expand_tilde(folder.trim());
    if !folder.is_dir() {
        bail!("{} is not a folder", folder.display());
    }
    Herdr::new(ctx.env.herdr_bin(), socket, ctx.runner)
        .workspace_create(&folder, label.trim(), true)
        .map(|_| ())
        .map_err(|e| anyhow::anyhow!("{e}"))
}

/// The agent pane of a workspace and its directory, for adopting it.
pub fn workspace_agent(ctx: &Ctx, workspace: &str) -> Result<(String, String)> {
    let socket = env_socket(ctx).context("HERDR_SOCKET_PATH is not set: open this from Herdr")?;
    let herdr = Herdr::new(ctx.env.herdr_bin(), socket, ctx.runner);
    let agents = herdr.agent_list_rpc().map_err(|e| anyhow::anyhow!("{e}"))?;
    let agent = agents
        .iter()
        .find(|a| a.workspace_id == workspace)
        .context("this workspace has no agent pane to continue")?;
    Ok((agent.pane_id.clone(), agent.cwd.clone()))
}

/// The exact CLI call a form would make, for its preview line.
pub fn preview_project(form: &NewProject) -> String {
    if !form.adopt_pane.is_empty() {
        return format!(
            "$ herdr-organizations adopt-workspace --name {:?} --pane {}",
            form.name, form.adopt_pane
        );
    }
    let mut line = format!("$ herdr-organizations new {:?}", form.name);
    if !form.goal.is_empty() {
        line.push_str(&format!(" --goal {:?}", form.goal));
    }
    for repo in form.repos.iter().filter(|r| !r.trim().is_empty()) {
        line.push_str(&format!(" --repo {}", repo.trim()));
    }
    line
}

pub fn preview_node(form: &NewNode) -> String {
    let mut line = format!(
        "$ herdr-organizations node start {} --parent {} --role {} --title {:?}",
        form.slug,
        form.parent,
        form.role.as_str(),
        form.title
    );
    for (flag, value) in [
        ("--template", &form.template),
        ("--repo", &form.repo),
        ("--harness", &form.harness),
        ("--model", &form.model),
        ("--reasoning-effort", &form.effort),
    ] {
        if !value.trim().is_empty() {
            line.push_str(&format!(" {flag} {}", value.trim()));
        }
    }
    line.push_str(" --task-file -");
    line
}

/// Closes an open node the way Nenu's Close does: the agent stops and its
/// Herdr view closes, while its worktree, branch and report stay.
pub fn close(ctx: &Ctx, slug: &str, id: &str) -> Result<()> {
    term::quiet(|| {
        threads::resolve(
            ctx,
            slug,
            id,
            &threads::ResolveArgs {
                keep_worktree: true,
                close_view: true,
                ..threads::ResolveArgs::default()
            },
        )
    })
}

/// Stops the project's running coordinator agents and starts a fresh one on
/// `profile`, or on `launch`'s harness, with its model and effort on top, in
/// the same session. The choice is checked before anything stops; the old
/// conversation ends with its pane.
pub fn replace_coordinator(
    ctx: &Ctx,
    slug: &str,
    profile: &str,
    launch: &agent_profile::ProfileOverrides,
) -> Result<String> {
    let project = Project::load(&ctx.root, slug)?;
    let socket = project
        .coordinator()
        .map(|record| PathBuf::from(record.socket))
        .filter(|socket| !socket.as_os_str().is_empty() && socket.exists())
        .or_else(|| env_socket(ctx))
        .context("no Herdr session to start the coordinator in")?;
    let name = if profile.is_empty() {
        launch.harness.clone().unwrap_or_default()
    } else {
        profile.to_string()
    };
    if name.is_empty() {
        bail!("choose the new coordinator's --profile or --harness");
    }
    let options = coordinator::OpenOptions {
        session: SessionFlags {
            session: None,
            socket: Some(socket.clone()),
        },
        rebind: true,
        profile: (!profile.is_empty()).then(|| profile.to_string()),
        launch: launch.clone(),
        new: true,
        here: false,
    };
    coordinator::check_launch(ctx, slug, &options)?;
    let herdr = Herdr::new(ctx.env.herdr_bin(), &socket, ctx.runner);
    let cwd = project.canonical_dir().to_string_lossy().into_owned();
    let running: Vec<String> = herdr
        .agent_list()
        .map_err(|e| anyhow::anyhow!("{e}"))?
        .into_iter()
        .filter(|a| a.works_in(&cwd))
        .map(|a| a.pane_id)
        .collect();
    for pane in &running {
        herdr
            .pane_close(pane)
            .map_err(|e| anyhow::anyhow!("could not stop the coordinator in {pane}: {e}"))?;
    }
    coordinator::open_quiet(ctx, slug, &options).context("could not start the new coordinator")?;
    let on = [
        Some(name),
        launch
            .model
            .clone()
            .filter(|m| !m.is_empty())
            .map(|m| format!("model {m}")),
        launch
            .reasoning_effort
            .clone()
            .filter(|e| !e.is_empty())
            .map(|e| format!("effort {e}")),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" · ");
    Ok(match running.len() {
        0 => format!("started a coordinator on {on}"),
        n => format!("stopped {n} coordinator agent(s) and started a new one on {on}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn previews_are_the_exact_cli_calls() {
        let project = NewProject {
            name: "Pricing panel".into(),
            repos: vec!["~/dev/app".into(), " ".into()],
            ..NewProject::default()
        };
        assert_eq!(
            preview_project(&project),
            "$ herdr-organizations new \"Pricing panel\" --repo ~/dev/app"
        );
        let node = NewNode {
            slug: "acme".into(),
            parent: "t-0001".into(),
            role: NodeRole::Worker,
            title: "billing ui".into(),
            harness: "claude".into(),
            ..NewNode::default()
        };
        assert_eq!(
            preview_node(&node),
            "$ herdr-organizations node start acme --parent t-0001 --role worker --title \"billing ui\" --harness claude --task-file -"
        );
    }

    /// Going to a node of a project that organizes coordinator nodes with
    /// no root one focuses the node and starts no root coordinator.
    #[test]
    fn going_to_a_node_of_a_project_without_a_root_coordinator_opens_none() {
        let world = crate::scenarios::World::new();
        let project = world.project("demo", "a.sock");
        project.update_coordinator(|c| c.pane_id.clear()).unwrap();
        let cwd = world.home.path().to_string_lossy().into_owned();
        let node = world.thread(&project, world.home.path(), |t| {
            t.kind = Kind::Adopted;
            t.agent_name = "helper".into();
        });
        *world.agents.borrow_mut() = format!(
            "[{}]",
            crate::scenarios::agent_json("w2", "w2:t1", "w2:p1", &cwd, "helper", "idle")
        );
        world
            .runner
            .on("agent focus", crate::runner::fake::ok(r#"{"result":{}}"#));

        let pane = go_to_pane(&world.ctx(), "demo", &node.id).unwrap();
        assert_eq!(pane, "w2:p1");
        assert_eq!(world.runner.count("agent focus"), 1);
        for started in ["agent start", "workspace create", "tab create"] {
            assert_eq!(world.runner.count(started), 0, "{started}");
        }
        assert!(project.coordinator().unwrap().pane_id.is_empty());
    }
}

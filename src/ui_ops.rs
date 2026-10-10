//! What the popup and the dock do when a key asks for it. Each is one user
//! action: these are the only places the views call Herdr, git or gh, and
//! every one reuses the CLI mechanic of the same name.

use std::path::PathBuf;

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

/// Squash-merges behind the same guard as `thread merge`, tells the
/// coordinator through the inbox, then resolves the thread: its worktree is
/// removed unless `keep_worktree`, otherwise only its Herdr view closes.
pub fn merge(ctx: &Ctx, slug: &str, id: &str, keep_worktree: bool) -> Result<String> {
    let project = Project::load(&ctx.root, slug)?;
    let record = thread::load(&project, id)?;
    threads::refuse_agent_pane(ctx, &project, "merging")?;
    steps::merge_pull_request(ctx, &project, id, pr::MergeMethod::Squash)?;
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
                        socket: Some(socket.clone()),
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
    let project = project::create(&ctx.root, &form.name, &form.goal, repos)?;
    // A Herdr kind is also the built-in profile of that name.
    crate::profiles::write_project_defaults(&project, &form.thread_agent, &form.coordinator_agent)?;
    term::quiet(|| {
        coordinator::open(
            ctx,
            &project.slug,
            &coordinator::OpenOptions {
                session: SessionFlags {
                    session: None,
                    socket: Some(socket),
                },
                rebind: false,
                profile: None,
                new: false,
                here: false,
            },
        )
    })?;
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
}

pub fn create_node(ctx: &Ctx, form: &NewNode) -> Result<Thread> {
    let blank = |s: &str| (!s.trim().is_empty()).then(|| s.trim().to_string());
    let task = if form.task.trim().is_empty() {
        form.title.clone()
    } else {
        form.task.clone()
    };
    term::quiet(|| {
        threads::start(
            ctx,
            &form.slug,
            threads::StartArgs {
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
                },
            },
        )
    })
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
}

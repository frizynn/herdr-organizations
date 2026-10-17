//! Adopting an already-running local agent pane into a project.

use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, bail};

use crate::herdr::{Agent, Herdr};
use crate::organizations::{self, NodeRequest};
use crate::paths::{self, Ctx, SessionFlags};
use crate::project::{self, Project};
use crate::runner::Cmd;
use crate::thread::{self, NodeRole, Status, Thread};
use crate::{coordinator, threads, ticker};

const DEFAULT_TASK: &str = "Continue the work you were already doing in this pane. It now belongs to the project described above: follow its instructions, and when you finish or stop to wait for the user, write the report.";

/// The refusals shared by `thread adopt` and `adopt-workspace`, checked before
/// anything is created. Returns the agent herdr detects in the pane.
pub fn adoptable_agent(ctx: &Ctx, herdr: &Herdr, socket: &str, pane: &str) -> Result<Agent> {
    let agents = herdr
        .agent_list()
        .map_err(|e| anyhow::anyhow!("the herdr session at {socket} is not reachable: {e}"))?;
    let agent = agents
        .into_iter()
        .find(|a| a.pane_id == pane)
        .with_context(|| format!("no agent is detected in pane {pane} of the session at {socket}; only a running local agent pane can be adopted"))?;
    // A pane is identified by (socket, machine, id): the same id in another
    // session is another pane.
    for slug in project::list_slugs(&ctx.root) {
        let Ok(other) = Project::load(&ctx.root, &slug) else {
            continue;
        };
        let Some(record) = other.coordinator() else {
            continue;
        };
        if record.socket != socket {
            continue;
        }
        if coordinator::is_coordinator(&record, &agent) {
            bail!(
                "pane {pane} is a coordinator of `{slug}` (its working directory is the project folder)"
            );
        }
        if let Some(t) = thread::list(&other).iter().find(|t| {
            !t.is_remote()
                && t.status != Status::Resolved
                && t.pane_id == pane
                && thread::agent_matches(t, &agent)
        }) {
            bail!("pane {pane} is already thread {} of `{slug}`", t.id);
        }
    }
    Ok(agent)
}

/// What `thread adopt` and `node adopt` record about a running pane.
#[derive(Debug, Clone, Default)]
pub struct AdoptArgs {
    pub pane: String,
    pub title: String,
    /// The task; none or blank continues the work already in the pane.
    pub task: Option<String>,
    /// `--parent`. `None` is the coordinator node whose pane runs the
    /// command, else root, as before the node flags existed.
    pub parent: Option<String>,
    pub role: NodeRole,
    /// `--can-spawn` or `--no-spawn`; unset, a coordinator may spawn.
    pub can_spawn: Option<bool>,
    /// Standing instructions written to the node's `INSTRUCTIONS.md`.
    pub rules: String,
    /// The session a project with no session record is tied to.
    pub session: SessionFlags,
}

pub fn adopt(ctx: &Ctx, slug: &str, args: &AdoptArgs) -> Result<Thread> {
    let project = Project::load(&ctx.root, slug)?;
    if project.status() != project::Status::Active {
        bail!(
            "`{slug}` is {}; adopting is refused until it is active again",
            project.status()
        );
    }
    if args.title.trim().is_empty() {
        bail!("--title may not be empty");
    }
    let (caller_pane, caller_socket) = (
        ctx.env.var("HERDR_PANE_ID").unwrap_or(""),
        ctx.env.var("HERDR_SOCKET_PATH").unwrap_or(""),
    );
    let parent_id = match &args.parent {
        Some(parent) => {
            organizations::authorize_creator(&project, caller_pane, caller_socket, parent)?;
            parent.clone()
        }
        None => organizations::creator_parent(&project, caller_pane, caller_socket)?
            .unwrap_or_else(|| organizations::ROOT_ID.into()),
    };
    let request = NodeRequest {
        parent_id,
        role: args.role,
        can_spawn: args.can_spawn,
        rules: args.rules.clone(),
        ..NodeRequest::default()
    };
    // Refused before Herdr is asked anything; checked again under the lock.
    organizations::check_adoption(&project, &request)?;
    let record = match project.coordinator() {
        Some(record) => record,
        None => coordinator::bind(ctx, &project, &args.session)?,
    };
    let herdr = Herdr::new(ctx.env.herdr_bin(), &record.socket, ctx.runner);
    let agent = adoptable_agent(ctx, &herdr, &record.socket, &args.pane)?;
    ticker::start(ctx)?;

    // The pane's repository and branch, when it is in one.
    let git = |args: &[&str]| -> Option<String> {
        let out = ctx
            .runner
            .run(
                &Cmd::new("git", Duration::from_secs(5))
                    .args(["-C", &agent.cwd])
                    .args(args.iter().copied()),
            )
            .ok()?;
        out.success()
            .then(|| out.stdout.trim().to_string())
            .filter(|s| !s.is_empty())
    };
    let repo = git(&["rev-parse", "--show-toplevel"]).unwrap_or_default();
    let branch = git(&["rev-parse", "--abbrev-ref", "HEAD"])
        .filter(|b| b != "HEAD")
        .unwrap_or_default();
    let origin = git(&["remote", "get-url", "origin"]).unwrap_or_default();

    let task = args
        .task
        .clone()
        .filter(|t| !t.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_TASK.to_string());
    let created = organizations::adopt_node(&project, &request, &task, |t| {
        t.title = args.title.trim().to_string();
        t.repo = repo;
        t.branch = branch;
        t.origin = origin;
        t.agent = agent.agent.clone();
        // Not started by the binary: whatever name herdr reports, possibly empty.
        t.agent_name = agent.name.clone();
        t.cwd = agent.cwd.clone();
        t.workspace_id = agent.workspace_id.clone();
        t.tab_id = agent.tab_id.clone();
        t.pane_id = agent.pane_id.clone();
    })?;
    let id = created.id.clone();

    let briefed = (|| -> Result<()> {
        // Two adopted panes in one directory still get separate thread directories.
        let dir = thread::thread_dir(&agent.cwd, slug, &id);
        let with_dir = Thread {
            thread_dir: dir.clone(),
            ..created.clone()
        };
        let brief = thread::brief_for(&project, &with_dir, &task, false)?;
        std::fs::create_dir_all(Path::new(&dir).join("library"))
            .with_context(|| format!("could not create {dir}"))?;
        threads::exclude_from_git(ctx.runner, &agent.cwd)?;
        project::write_atomic(&Path::new(&dir).join("brief.md"), brief.as_bytes())?;
        thread::update(&project, &id, |t| t.thread_dir = dir)?;
        Ok(())
    })();
    if let Err(error) = briefed {
        let message = format!("{error:#}");
        let _ = thread::update(&project, &id, |t| {
            t.status = Status::Failed;
            t.error = message;
        });
        return Err(error);
    }

    let open = thread::update(&project, &id, |t| {
        t.status = Status::Open;
        t.prompt_pending = true;
        t.last_state = agent.agent_status.clone();
        t.last_state_change = project::now();
    })?;
    // Prompt now when the agent is ready for one and its box is empty (a
    // draft is never merged into the brief); otherwise the ticker's delivery
    // path sends the line later (also when the agent ends in `done`).
    if agent.ready() {
        crate::brief::deliver(
            &project,
            &herdr,
            &open,
            &agent,
            crate::brief::Sender::Manual,
        )?;
    }
    let adopted = thread::load(&project, &id)?;
    threads::report_thread_tokens(&herdr, &adopted, slug, thread::Group::Working);
    Ok(adopted)
}

pub struct AdoptWorkspace {
    pub name: String,
    pub goal: String,
    pub pane: String,
    pub workspace_cwd: String,
    pub session: SessionFlags,
}

/// "Continue as a project": `new`, then `open`, then `thread adopt`. It
/// refuses, before creating anything, under the same conditions as `thread adopt`.
pub fn adopt_workspace(ctx: &Ctx, args: &AdoptWorkspace) -> Result<()> {
    let session = paths::resolve_session(&args.session, ctx.env, ctx.runner)?;
    let socket = session.socket.to_string_lossy().into_owned();
    let herdr = Herdr::new(ctx.env.herdr_bin(), &session.socket, ctx.runner);
    let agent = adoptable_agent(ctx, &herdr, &socket, &args.pane)?;
    let slug = project::slug_from_name(&args.name)?;
    if ctx.root.join(&slug).exists() {
        bail!("`{slug}` already exists in {}", ctx.root.display());
    }

    // repo = the workspace's directory when that is a git repository.
    let cwd = if args.workspace_cwd.is_empty() {
        agent.cwd.clone()
    } else {
        args.workspace_cwd.clone()
    };
    let is_repo = ctx
        .runner
        .run(&Cmd::new("git", Duration::from_secs(5)).args([
            "-C",
            &cwd,
            "rev-parse",
            "--show-toplevel",
        ]))
        .ok()
        .filter(|o| o.success())
        .map(|o| o.stdout.trim().to_string())
        .filter(|s| !s.is_empty());
    let repos = is_repo
        .map(|path| {
            vec![project::Repo {
                path,
                machine: None,
            }]
        })
        .unwrap_or_default();

    let project = project::create_for(&ctx.config_dir, &ctx.root, &args.name, &args.goal, repos)?;
    let config = crate::profiles::load(&ctx.config_dir)?;
    crate::profiles::write_project_defaults(
        &project,
        &config.new_project_default(crate::profiles::Role::Thread),
        &config.new_project_default(crate::profiles::Role::Coordinator),
    )?;
    project::write_priming(&project, &coordinator::current_prefix(&ctx.root)?)?;
    println!("created `{}` at {}", project.slug, project.dir().display());
    coordinator::open(
        ctx,
        &project.slug,
        &coordinator::OpenOptions {
            session: SessionFlags {
                session: None,
                socket: Some(session.socket.clone()),
            },
            rebind: false,
            profile: None,
            launch: Default::default(),
            new: false,
            here: false,
        },
    )?;
    let adopted = adopt(
        ctx,
        &project.slug,
        &AdoptArgs {
            pane: args.pane.clone(),
            title: args.name.clone(),
            ..AdoptArgs::default()
        },
    )?;
    println!(
        "adopted pane {} as thread {} of `{}`",
        args.pane, adopted.id, project.slug
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::organizations::{CreateNode, NodeRequest, ROOT_ID};
    use crate::paths::Env;
    use crate::runner::fake::{fail, ok};
    use crate::scenarios::{World, agent_json};
    use crate::thread::Kind;

    fn world_with_agent(state: &str, name: &str) -> (World, Project, String) {
        let world = World::new();
        let project = world.project("demo", "a.sock");
        let cwd = world.home.path().join("work");
        std::fs::create_dir(&cwd).unwrap();
        let cwd = cwd.to_string_lossy().into_owned();
        *world.agents.borrow_mut() = format!(
            "[{},{}]",
            agent_json("w5", "w5:t1", "w5:p1", &cwd, name, state),
            agent_json("w5", "w5:t1", "w5:p2", &cwd, "", "idle")
        );
        world.runner.on("git -C", fail(128, "not a git repository"));
        world.runner.on("agent prompt", ok(r#"{"result":{}}"#));
        (world, project, cwd)
    }

    /// `thread adopt` as it was before the node flags: under the caller's
    /// node, else root, as a worker.
    fn plain(pane: &str, title: &str, task: Option<String>) -> AdoptArgs {
        AdoptArgs {
            pane: pane.into(),
            title: title.into(),
            task,
            ..AdoptArgs::default()
        }
    }

    fn under(pane: &str, title: &str, parent: &str, role: NodeRole) -> AdoptArgs {
        AdoptArgs {
            parent: Some(parent.into()),
            role,
            ..plain(pane, title, None)
        }
    }

    /// Three idle agents in one session: `w5:p1` (a coordinator someone
    /// started by hand), `w5:p2` and `w5:p3` (its agents).
    fn world_with_three_agents() -> (World, Project, String) {
        let (world, project, cwd) = world_with_agent("idle", "boss");
        *world.agents.borrow_mut() = format!(
            "[{},{},{}]",
            agent_json("w5", "w5:t1", "w5:p1", &cwd, "boss", "idle"),
            agent_json("w5", "w5:t2", "w5:p2", &cwd, "helper-a", "idle"),
            agent_json("w5", "w5:t3", "w5:p3", &cwd, "helper-b", "idle")
        );
        (world, project, cwd)
    }

    fn from_pane(world: &World, project: &Project, pane: &str) -> Env {
        let socket = project.coordinator().unwrap().socket;
        Env::for_test(
            world.home.path(),
            &[
                ("HERDR_PANE_ID", pane),
                ("HERDR_SOCKET_PATH", socket.as_str()),
            ],
        )
    }

    /// A coordinator already running becomes a coordinator node under root
    /// and its agents become its children, adopted from outside with
    /// `--parent` and from its own pane without it. Nothing is started.
    #[test]
    fn a_running_coordinator_and_its_agents_become_a_node_and_its_children() {
        let (world, project, _) = world_with_three_agents();
        let ctx = world.ctx();
        let coordinator = adopt(
            &ctx,
            "demo",
            &under("w5:p1", "Area", ROOT_ID, NodeRole::Coordinator),
        )
        .unwrap();
        assert_eq!(
            (
                coordinator.kind,
                coordinator.parent_id.as_str(),
                coordinator.role,
                coordinator.can_spawn,
                coordinator.pane_id.as_str(),
                coordinator.status
            ),
            (
                Kind::Adopted,
                ROOT_ID,
                NodeRole::Coordinator,
                true,
                "w5:p1",
                Status::Open
            )
        );
        // Its status file's folder is ready for it.
        let records = project.records_dir();
        assert!(
            crate::records::node_status(&records, &coordinator.id)
                .parent()
                .unwrap()
                .is_dir()
        );

        let outside = adopt(
            &ctx,
            "demo",
            &under("w5:p2", "Helper A", &coordinator.id, NodeRole::Worker),
        )
        .unwrap();
        assert_eq!(outside.parent_id, coordinator.id);
        assert_eq!(outside.role, NodeRole::Worker);

        let env = from_pane(&world, &project, "w5:p1");
        let own_pane = Ctx {
            env: &env,
            ..world.ctx()
        };
        let inside = adopt(&own_pane, "demo", &plain("w5:p3", "Helper B", None)).unwrap();
        assert_eq!(inside.parent_id, coordinator.id);

        let tree: Vec<(String, usize)> = organizations::tree(&project)
            .unwrap()
            .into_iter()
            .map(|entry| (entry.thread.id, entry.depth))
            .collect();
        assert_eq!(
            tree,
            [
                ("t-0001".to_string(), 1),
                ("t-0002".to_string(), 2),
                ("t-0003".to_string(), 2)
            ]
        );
        assert_eq!(world.runner.count("agent start"), 0);
        assert_eq!(world.runner.count("agent prompt"), 3);
    }

    /// An adopted coordinator that may spawn gets the coordinator-node
    /// brief, which first tells it, in fixed words, that it keeps going and
    /// that its agents are now its children. A worker's brief has none of it.
    #[test]
    fn an_adopted_coordinator_is_told_to_carry_on_with_its_agents_as_children() {
        let (world, _, _) = world_with_three_agents();
        let ctx = world.ctx();
        let coordinator = adopt(
            &ctx,
            "demo",
            &under("w5:p1", "Area", ROOT_ID, NodeRole::Coordinator),
        )
        .unwrap();
        let brief =
            std::fs::read_to_string(format!("{}/brief.md", coordinator.thread_dir)).unwrap();
        let prefix = coordinator::current_prefix(&ctx.root).unwrap();
        let preface = format!(
            "You were already working in this pane before it joined this project as coordinator node `t-0001`. Carry on with what you had in progress. From now on the agents you run are your child nodes: adopt each one that is already running with `{prefix} node adopt demo --pane <pane id> --title \"Short title\"`, which places it under you, and start new ones with `node start` as described below. Keep your state in your status file, not in chat.\n\n"
        );
        assert!(brief.starts_with(&preface), "{brief}");
        assert!(brief.contains("# Coordinator node brief"), "{brief}");
        assert!(!brief.contains("# Thread brief"), "{brief}");
        assert!(brief.contains("# Your status"), "{brief}");
        assert_eq!(brief.matches("You were already working").count(), 1);

        let worker = adopt(
            &ctx,
            "demo",
            &under("w5:p2", "Helper", &coordinator.id, NodeRole::Worker),
        )
        .unwrap();
        let brief = std::fs::read_to_string(format!("{}/brief.md", worker.thread_dir)).unwrap();
        assert!(brief.starts_with("# Project\n"), "{brief}");
        assert!(brief.contains("# Thread brief"), "{brief}");
        assert!(!brief.contains("You were already working"), "{brief}");
    }

    /// The parent checks of `node start`, refused before anything is
    /// created and before Herdr is asked anything.
    #[test]
    fn adopting_refuses_the_parents_node_start_refuses() {
        let (world, project, _) = world_with_three_agents();
        let ctx = world.ctx();
        let worker = organizations::create_node(
            &project,
            &CreateNode {
                request: NodeRequest::default(),
                title: "Worker".into(),
                kind: Kind::Tab,
                repo: String::new(),
                machine: String::new(),
                base: String::new(),
                task: "task".into(),
            },
        )
        .unwrap();
        let leaf = organizations::create_node(
            &project,
            &CreateNode {
                request: NodeRequest {
                    role: NodeRole::Coordinator,
                    can_spawn: Some(false),
                    ..NodeRequest::default()
                },
                title: "Leaf".into(),
                kind: Kind::Tab,
                repo: String::new(),
                machine: String::new(),
                base: String::new(),
                task: "task".into(),
            },
        )
        .unwrap();
        let area = organizations::create_node(
            &project,
            &CreateNode {
                request: NodeRequest {
                    role: NodeRole::Coordinator,
                    ..NodeRequest::default()
                },
                title: "Area".into(),
                kind: Kind::Tab,
                repo: String::new(),
                machine: String::new(),
                base: String::new(),
                task: "task".into(),
            },
        )
        .unwrap();
        thread::update(&project, &area.id, |t| t.pane_id = "w6:p1".into()).unwrap();
        let before = thread::list(&project);
        let env = from_pane(&world, &project, "w6:p1");
        let area_pane = Ctx {
            env: &env,
            ..world.ctx()
        };
        let refusals: [(&Ctx, AdoptArgs, &str); 5] = [
            (
                &ctx,
                under("w5:p1", "x", &worker.id, NodeRole::Worker),
                "worker node `t-0001` cannot spawn children",
            ),
            (
                &ctx,
                under("w5:p1", "x", &leaf.id, NodeRole::Worker),
                "node `t-0002` does not have permission to spawn children",
            ),
            (
                &ctx,
                under("w5:p1", "x", "t-0099", NodeRole::Worker),
                "parent node `t-0099` does not exist",
            ),
            (
                &ctx,
                AdoptArgs {
                    can_spawn: Some(true),
                    ..under("w5:p1", "x", ROOT_ID, NodeRole::Worker)
                },
                "worker nodes cannot spawn children",
            ),
            (
                &area_pane,
                under("w5:p1", "x", ROOT_ID, NodeRole::Worker),
                "coordinator `t-0003` may create children only beneath itself, not `root`",
            ),
        ];
        for (ctx, args, expected) in refusals {
            let error = adopt(ctx, "demo", &args).unwrap_err().to_string();
            assert!(error.contains(expected), "{error}");
        }
        assert_eq!(thread::list(&project), before);
        assert_eq!(world.runner.calls.borrow().len(), 0);

        // A pane already adopted, and one with no agent, as a coordinator too.
        adopt(
            &ctx,
            "demo",
            &under("w5:p1", "Boss", ROOT_ID, NodeRole::Coordinator),
        )
        .unwrap();
        let again = adopt(
            &ctx,
            "demo",
            &under("w5:p1", "Again", ROOT_ID, NodeRole::Coordinator),
        )
        .unwrap_err()
        .to_string();
        assert!(again.contains("already thread t-0004"), "{again}");
        let shell = adopt(
            &ctx,
            "demo",
            &under("w9:p9", "Shell", ROOT_ID, NodeRole::Coordinator),
        )
        .unwrap_err()
        .to_string();
        assert!(shell.contains("no agent is detected"), "{shell}");
        assert_eq!(thread::list(&project).len(), 4);
    }

    /// A tree that is already broken refuses an adoption and is left as it was.
    #[test]
    fn adopting_into_a_tree_with_a_cycle_is_refused() {
        let (world, project, _) = world_with_three_agents();
        for (id, parent) in [("t-0001", "t-0002"), ("t-0002", "t-0001")] {
            let record = Thread {
                id: id.into(),
                parent_id: parent.into(),
                role: NodeRole::Coordinator,
                can_spawn: true,
                ..Thread::default()
            };
            std::fs::write(
                thread::record_path(&project, id),
                toml::to_string(&record).unwrap(),
            )
            .unwrap();
        }
        let error = adopt(
            &world.ctx(),
            "demo",
            &under("w5:p1", "x", "t-0001", NodeRole::Worker),
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("cycle"), "{error}");
        assert_eq!(thread::list(&project).len(), 2);
    }

    /// `--rules-file` writes the node's standing instructions with the record.
    #[test]
    fn adopting_with_rules_writes_them_to_the_nodes_instructions() {
        let (world, project, _) = world_with_three_agents();
        let node = adopt(
            &world.ctx(),
            "demo",
            &AdoptArgs {
                rules: "Answer in Spanish.".into(),
                ..under("w5:p1", "Area", ROOT_ID, NodeRole::Coordinator)
            },
        )
        .unwrap();
        assert_eq!(
            organizations::instruction_rules(
                &organizations::node_rules(&project, &node.id).unwrap()
            ),
            "Answer in Spanish."
        );
        let brief = std::fs::read_to_string(format!("{}/brief.md", node.thread_dir)).unwrap();
        assert!(brief.contains("Answer in Spanish."), "{brief}");
    }

    /// Without the new flags, the record, the files and the brief are the
    /// ones `thread adopt` wrote before them: a worker under root, no node
    /// scope, the thread brief.
    #[test]
    fn thread_adopt_without_the_node_flags_is_unchanged() {
        let (world, project, _) = world_with_agent("idle", "my-agent");
        let t = adopt(&world.ctx(), "demo", &plain("w5:p1", "Adopted", None)).unwrap();
        assert_eq!(
            (t.parent_id.as_str(), t.role, t.can_spawn, t.kind),
            (ROOT_ID, NodeRole::Worker, false, Kind::Adopted)
        );
        assert!(!organizations::node_scope_dir(&project, &t.id).exists());
        assert_eq!(
            std::fs::read_to_string(thread::task_path(&project, &t.id)).unwrap(),
            DEFAULT_TASK
        );
        let brief = std::fs::read_to_string(format!("{}/brief.md", t.thread_dir)).unwrap();
        assert_eq!(
            brief,
            thread::brief_for(&project, &t, DEFAULT_TASK, false).unwrap()
        );
        assert!(brief.starts_with("# Project\n"), "{brief}");
        // The project's session record is the one `open` wrote.
        assert_eq!(project.coordinator().unwrap().pane_id, "w1:p1");
        assert_eq!(world.runner.count("workspace create"), 0);
    }

    /// A project with no session record is tied to the caller's session: its
    /// home Space is made with a shell and no agent, and the record names no
    /// root coordinator. The next adoption reuses it.
    #[test]
    fn adopting_into_a_project_never_opened_binds_it_to_the_callers_session() {
        let world = World::new();
        let project = project::create(&world.root, "demo", "", vec![]).unwrap();
        let socket = world.home.path().join("a.sock");
        std::fs::write(&socket, b"").unwrap();
        let socket = socket.to_string_lossy().into_owned();
        let home = project.canonical_dir().to_string_lossy().into_owned();
        let cwd = world.home.path().join("work");
        std::fs::create_dir(&cwd).unwrap();
        let cwd = cwd.to_string_lossy().into_owned();
        *world.agents.borrow_mut() = format!(
            "[{},{}]",
            agent_json("w5", "w5:t1", "w5:p1", &cwd, "boss", "idle"),
            agent_json("w5", "w5:t1", "w5:p2", &cwd, "helper", "idle")
        );
        *world.panes.borrow_mut() = format!(
            "[{}]",
            crate::scenarios::pane_json("w5", "w5:t1", "w5:p1", &cwd)
        );
        let panes = world.panes.clone();
        let created_home = home.clone();
        world.runner.on_fn(
            |cmd| cmd.display().contains("workspace create"),
            move |_| {
                *panes.borrow_mut() = format!(
                    "[{},{}]",
                    crate::scenarios::pane_json("w5", "w5:t1", "w5:p1", "/elsewhere"),
                    crate::scenarios::pane_json("w7", "w7:t1", "w7:p1", &created_home)
                );
                Ok(ok(
                    r#"{"result":{"root_pane":{"workspace_id":"w7","tab_id":"w7:t1","pane_id":"w7:p1"}}}"#,
                ))
            },
        );
        world.runner.on("git -C", fail(128, "not a git repository"));
        world.runner.on("agent prompt", ok(r#"{"result":{}}"#));
        let env = Env::for_test(
            world.home.path(),
            &[("HERDR_PANE_ID", "w5:p9"), ("HERDR_SOCKET_PATH", &socket)],
        );
        let ctx = Ctx {
            env: &env,
            ..world.ctx()
        };

        let coordinator = adopt(
            &ctx,
            "demo",
            &under("w5:p1", "Boss", ROOT_ID, NodeRole::Coordinator),
        )
        .unwrap();
        let record = project.coordinator().unwrap();
        assert_eq!(
            (
                record.socket.as_str(),
                record.workspace_id.as_str(),
                record.pane_id.as_str(),
                record.agent.as_str(),
                record.cwd.as_str()
            ),
            (socket.as_str(), "w7", "", "", home.as_str())
        );
        assert_eq!(world.runner.count("workspace create"), 1);
        assert_eq!(world.runner.count("agent start"), 0);

        adopt(
            &ctx,
            "demo",
            &under("w5:p2", "Helper", &coordinator.id, NodeRole::Worker),
        )
        .unwrap();
        assert_eq!(world.runner.count("workspace create"), 1);
        assert_eq!(project.coordinator().unwrap(), record);
    }

    #[test]
    fn adopting_a_ready_agent_writes_a_brief_and_prompts_it() {
        let (world, project, cwd) = world_with_agent("idle", "my-agent");
        let t = adopt(
            &world.ctx(),
            "demo",
            &plain("w5:p1", "Adopted work", Some("Finish the refactor.".into())),
        )
        .unwrap();
        assert_eq!(
            (t.kind, t.status, t.prompt_pending),
            (Kind::Adopted, Status::Open, false)
        );
        assert_eq!(t.agent_name, "my-agent");
        assert_eq!(t.parent_id, ROOT_ID);
        assert_eq!(t.role, NodeRole::Worker);
        assert!(!t.can_spawn);
        assert_eq!(t.thread_dir, format!("{cwd}/.herdr-project/demo-t-0001"));
        let brief = std::fs::read_to_string(format!("{}/brief.md", t.thread_dir)).unwrap();
        assert!(brief.contains("Finish the refactor."));
        assert_eq!(world.runner.count("agent prompt"), 1);
        assert_eq!(world.runner.count("agent start"), 0);

        // A second pane in the same directory gets its own thread directory.
        let second = adopt(&world.ctx(), "demo", &plain("w5:p2", "Second", None)).unwrap();
        assert_eq!(
            second.thread_dir,
            format!("{cwd}/.herdr-project/demo-t-0002")
        );
        assert!(second.agent_name.is_empty());
        assert_ne!(t.thread_dir, second.thread_dir);
        let _ = project;
    }

    #[test]
    fn a_known_coordinator_adopts_under_its_own_node() {
        let (world, project, cwd) = world_with_agent("idle", "target-agent");
        let coordinator = organizations::create_node(
            &project,
            &CreateNode {
                request: NodeRequest {
                    parent_id: ROOT_ID.into(),
                    role: NodeRole::Coordinator,
                    ..NodeRequest::default()
                },
                title: "Area coordinator".into(),
                kind: Kind::Tab,
                repo: String::new(),
                machine: String::new(),
                base: String::new(),
                task: "Coordinate this area".into(),
            },
        )
        .unwrap();
        thread::update(&project, &coordinator.id, |record| {
            record.pane_id = "w6:p1".into();
        })
        .unwrap();
        *world.agents.borrow_mut() = format!(
            "[{},{}]",
            agent_json("w6", "w6:t1", "w6:p1", &cwd, "area-coordinator", "idle"),
            agent_json("w5", "w5:t1", "w5:p1", &cwd, "target-agent", "idle")
        );
        let socket = project.coordinator().unwrap().socket;
        let env = Env::for_test(
            world.home.path(),
            &[
                ("HERDR_PANE_ID", "w6:p1"),
                ("HERDR_SOCKET_PATH", socket.as_str()),
            ],
        );
        let ctx = Ctx {
            env: &env,
            ..world.ctx()
        };

        let adopted = adopt(&ctx, "demo", &plain("w5:p1", "Adopted", None)).unwrap();
        assert_eq!(adopted.parent_id, coordinator.id);
    }

    #[test]
    fn a_known_worker_cannot_adopt_a_thread_or_create_anything() {
        let (world, project, _) = world_with_agent("idle", "target-agent");
        let worker = world.thread(&project, world.home.path(), |_| {});
        let socket = project.coordinator().unwrap().socket;
        let env = Env::for_test(
            world.home.path(),
            &[
                ("HERDR_PANE_ID", worker.pane_id.as_str()),
                ("HERDR_SOCKET_PATH", socket.as_str()),
            ],
        );
        let ctx = Ctx {
            env: &env,
            ..world.ctx()
        };

        let error = adopt(&ctx, "demo", &plain("w5:p1", "Rejected", None))
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("worker nodes cannot create children"),
            "{error}"
        );
        assert_eq!(thread::list(&project).len(), 1);
        assert_eq!(world.runner.calls.borrow().len(), 0);
    }

    #[test]
    fn a_busy_agent_gets_its_prompt_later_even_when_it_ends_in_done() {
        let (world, project, cwd) = world_with_agent("working", "my-agent");
        let t = adopt(&world.ctx(), "demo", &plain("w5:p1", "Busy", None)).unwrap();
        assert!(t.prompt_pending);
        assert_eq!(world.runner.count("agent prompt"), 0);

        *world.agents.borrow_mut() = format!(
            "[{}]",
            agent_json("w5", "w5:t1", "w5:p1", &cwd, "my-agent", "done")
        );
        crate::ticker::tick_project(&world.ctx(), &project).unwrap();
        crate::scenarios::settled(&project);
        crate::ticker::tick_project(&world.ctx(), &project).unwrap();
        assert_eq!(world.runner.count("agent prompt"), 1);
        assert!(!thread::load(&project, "t-0001").unwrap().prompt_pending);
        // The ticker never tries to start an agent in an adopted pane it found busy.
        assert_eq!(world.runner.count("agent start"), 0);
    }

    #[test]
    fn refusals_happen_before_anything_is_created() {
        let (world, project, _) = world_with_agent("idle", "my-agent");
        let ctx = world.ctx();
        // No detected agent in that pane.
        assert!(
            adopt(&ctx, "demo", &plain("w9:p9", "x", None))
                .unwrap_err()
                .to_string()
                .contains("no agent is detected")
        );
        // Already a thread.
        adopt(&ctx, "demo", &plain("w5:p1", "first", None)).unwrap();
        assert!(
            adopt(&ctx, "demo", &plain("w5:p1", "again", None))
                .unwrap_err()
                .to_string()
                .contains("already thread t-0001")
        );
        assert_eq!(thread::list(&project).len(), 1);

        // Same pane id recorded by a project in ANOTHER socket is a different pane.
        let other = world.project("other", "b.sock");
        let t = adopt(&ctx, "other", &plain("w5:p1", "other session", None));
        // `other` lives in b.sock; the fake serves the same agent list for it, so
        // the pane is adoptable there: ids are only compared within one socket.
        assert!(t.is_ok(), "{t:?}");
        let _ = other;
    }

    #[test]
    fn adopt_workspace_refuses_without_an_agent_and_creates_nothing() {
        let (world, _, _) = world_with_agent("idle", "my-agent");
        let socket = world.home.path().join("a.sock");
        let args = AdoptWorkspace {
            name: "From Workspace".into(),
            goal: String::new(),
            pane: "w9:p9".into(),
            workspace_cwd: String::new(),
            session: SessionFlags {
                session: None,
                socket: Some(socket),
            },
        };
        assert!(adopt_workspace(&world.ctx(), &args).is_err());
        assert!(!world.root.join("from-workspace").exists());
    }
}

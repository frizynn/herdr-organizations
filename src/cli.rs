use std::path::PathBuf;

use anyhow::{Result, bail};
use clap::{Args, Parser, Subcommand};

use crate::agent_profile::ProfileOverrides;
use crate::coordinator::{self, OpenOptions};
use crate::organizations::NodeRequest;
use crate::paths::{self, Ctx, Env, SessionFlags};
use crate::project::{self, Project, Status};
use crate::runner::RealRunner;
use crate::thread::NodeRole;
use crate::threads::{self, ResolveArgs, StartArgs};
use crate::{actions, adopt, doctor, inbox, lifecycle, overview, routine, ticker};

#[derive(Parser)]
#[command(name = env!("CARGO_BIN_NAME"), version = crate::VERSION, about = "Recursive organizations for herdr")]
struct Cli {
    /// Projects root (default: $HERDR_PROJECTS_ROOT, then config.toml, then ~/.herdr-projects)
    #[arg(long, global = true, value_name = "DIR")]
    root: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Args, Clone, Default)]
pub struct SessionArgs {
    /// herdr session name
    #[arg(long, value_name = "NAME", conflicts_with = "socket")]
    session: Option<String>,
    /// herdr socket path
    #[arg(long, value_name = "PATH")]
    socket: Option<PathBuf>,
}

impl From<SessionArgs> for SessionFlags {
    fn from(args: SessionArgs) -> Self {
        SessionFlags {
            session: args.session,
            socket: args.socket,
        }
    }
}

#[derive(Subcommand)]
enum Command {
    /// Create a project folder with its skeleton files
    New {
        name: String,
        #[arg(long, default_value = "")]
        goal: String,
        /// A repository, as PATH or PATH@MACHINE; repeatable
        #[arg(long = "repo", value_name = "PATH[@MACHINE]")]
        repos: Vec<String>,
    },
    /// List projects
    List {
        /// Include archived projects
        #[arg(long)]
        all: bool,
    },
    /// Open a project: its workspace, coordinator tab and coordinator agent
    Open {
        slug: String,
        /// Send the priming prompt again
        #[arg(long)]
        reprime: bool,
        /// Move the project to this session when its recorded socket no longer exists
        #[arg(long)]
        rebind: bool,
        #[command(flatten)]
        session: SessionArgs,
    },
    /// Print the digest the coordinator reads at the start of every turn
    Context {
        slug: String,
        /// Print without recording the inbox items as seen
        #[arg(long)]
        peek: bool,
    },
    /// Print threads grouped by what needs you
    Overview {
        slug: Option<String>,
        /// Wait for Enter before exiting (only when on a terminal; used by the popup)
        #[arg(long)]
        wait: bool,
    },
    /// Show only one project's panes in the sidebar, sorted by attention
    Focus { slug: Option<String> },
    /// Clear the sidebar view (herdr holds one, so this clears any tool's view)
    Unfocus {
        #[command(flatten)]
        session: SessionArgs,
    },
    /// Inbox items
    Inbox {
        #[command(subcommand)]
        command: InboxCommand,
    },
    /// Threads: the project's worker agents
    Thread {
        #[command(subcommand)]
        command: ThreadCommand,
    },
    /// Create and manage nodes in a recursive organization
    Node {
        #[command(subcommand)]
        command: NodeCommand,
    },
    /// Routines: scheduled prompts and watched commands
    Routine {
        #[command(subcommand)]
        command: RoutineCommand,
    },
    /// Pause a project: the ticker skips it and `thread start` is refused
    Pause { slug: String },
    /// Make a paused project active again
    Resume { slug: String },
    /// Archive a project: paused, hidden, tokens cleared, `open` refused
    Archive { slug: String },
    /// Make an archived project active again
    Unarchive { slug: String },
    /// Move a project folder to the trash (no worktree, branch or PR is touched)
    Delete {
        slug: String,
        /// Delete even though coordinator or thread panes are alive
        #[arg(long)]
        force: bool,
    },
    /// Continue the current workspace's agent pane as a new project
    AdoptWorkspace {
        /// Project name (default: the workspace label herdr passes to the action)
        #[arg(long)]
        name: String,
        #[arg(long, default_value = "")]
        goal: String,
        /// The agent pane to adopt
        #[arg(long)]
        pane: String,
        /// The workspace's directory (the project's repo when it is a git repository)
        #[arg(long, default_value = "")]
        workspace_cwd: String,
        #[command(flatten)]
        session: SessionArgs,
    },
    /// Run by herdr's action menu
    #[command(hide = true)]
    Action { id: String },
    /// Run inside a plugin popup pane
    #[command(hide = true)]
    Pane { id: String },
    /// Safety settings
    Safety {
        #[command(subcommand)]
        command: SafetyCommand,
    },
    /// Print the coordinator skill
    Skill,
    /// Check the setup: versions, tools, root, ticker and each project's session
    Doctor {
        #[command(flatten)]
        session: SessionArgs,
    },
    /// The background ticker
    Ticker {
        #[command(subcommand)]
        command: TickerCommand,
    },
}

#[derive(Subcommand)]
enum InboxCommand {
    /// Move handled items to inbox/done/
    Done {
        slug: String,
        #[arg(value_name = "ITEM_ID", required_unless_present = "all")]
        ids: Vec<String>,
        #[arg(long, conflicts_with = "ids")]
        all: bool,
    },
}

#[derive(Subcommand)]
enum ThreadCommand {
    /// Start a thread: a worktree workspace for --repo, else a tab in the project workspace
    Start {
        slug: String,
        #[arg(long)]
        title: String,
        #[arg(long, value_name = "PATH")]
        repo: Option<String>,
        #[arg(long, value_name = "LABEL")]
        machine: Option<String>,
        /// Agent kind (default: thread_agent in PROJECT.md)
        #[arg(long, value_name = "KIND")]
        agent: Option<String>,
        #[arg(long, value_name = "REF")]
        base: Option<String>,
        /// The task; `-` reads standard input
        #[arg(long, value_name = "FILE")]
        task_file: String,
    },
    /// Bring back a thread whose pane is gone or whose start failed
    Restart { slug: String, id: String },
    /// Send a follow-up to a thread's agent
    Prompt {
        slug: String,
        id: String,
        /// The text; `-` reads standard input
        #[arg(long, value_name = "FILE")]
        text_file: String,
    },
    /// List threads with live state and group
    List { slug: String },
    /// Show one thread's record
    Show { slug: String, id: String },
    /// Record an existing local agent pane as a thread of this project
    Adopt {
        slug: String,
        #[arg(long, value_name = "ID")]
        pane: String,
        #[arg(long)]
        title: String,
        /// Optional task; `-` reads standard input
        #[arg(long, value_name = "FILE")]
        task_file: Option<String>,
    },
    /// Record that the user has seen the current report
    Ack { slug: String, id: String },
    /// Resolve a thread (final copy first), or reopen a resolved one
    Resolve {
        slug: String,
        id: String,
        #[arg(long, conflicts_with_all = ["remove_worktree", "skip_copy", "discard_uncopied"])]
        reopen: bool,
        /// Also remove the worktree (never forced; the branch is kept)
        #[arg(long)]
        remove_worktree: bool,
        /// Resolve even though the final copy cannot be made
        #[arg(long)]
        skip_copy: bool,
        /// With --remove-worktree: accept losing what could not be copied
        #[arg(long, requires = "remove_worktree")]
        discard_uncopied: bool,
    },
}

#[derive(clap::ValueEnum, Clone, Copy)]
enum CliNodeRole {
    Worker,
    Coordinator,
}

impl From<CliNodeRole> for NodeRole {
    fn from(value: CliNodeRole) -> Self {
        match value {
            CliNodeRole::Worker => NodeRole::Worker,
            CliNodeRole::Coordinator => NodeRole::Coordinator,
        }
    }
}

#[derive(Args)]
struct NodeStartArgs {
    slug: String,
    #[arg(long)]
    title: String,
    /// Parent node id, or `root` for a direct child of the project
    #[arg(long, default_value = "root")]
    parent: String,
    /// Coordinators can create children; workers cannot
    #[arg(long, value_enum, default_value_t = CliNodeRole::Worker)]
    role: CliNodeRole,
    /// Explicitly grant a coordinator permission to create children
    #[arg(long, conflicts_with = "no_spawn")]
    can_spawn: bool,
    /// Create a leaf coordinator that cannot create children
    #[arg(long = "no-spawn", conflicts_with = "can_spawn")]
    no_spawn: bool,
    #[arg(long, value_name = "HARNESS", visible_alias = "agent")]
    harness: Option<String>,
    #[arg(long)]
    model: Option<String>,
    #[arg(long)]
    reasoning_effort: Option<String>,
    #[arg(long)]
    permission_profile: Option<String>,
    /// One argv component passed directly to the selected harness; repeatable
    #[arg(long = "raw-agent-arg")]
    raw_agent_args: Vec<String>,
    #[arg(long, value_name = "PATH")]
    repo: Option<String>,
    #[arg(long, value_name = "LABEL")]
    machine: Option<String>,
    #[arg(long, value_name = "REF")]
    base: Option<String>,
    /// The task; `-` reads standard input
    #[arg(long, value_name = "FILE")]
    task_file: String,
}

#[derive(Subcommand)]
enum NodeCommand {
    /// Create and start a child node; `create` is an alias
    #[command(alias = "create")]
    Start(Box<NodeStartArgs>),
    /// Bring back a node whose pane is gone or whose start failed
    Restart { slug: String, id: String },
    /// Send a follow-up to a node's agent
    Prompt {
        slug: String,
        id: String,
        #[arg(long, value_name = "FILE")]
        text_file: String,
    },
    /// List nodes with live state, role and parent
    List { slug: String },
    /// Show one node's record
    Show { slug: String, id: String },
    /// Record that the user has seen the current report
    Ack { slug: String, id: String },
    /// Resolve a node, or reopen a resolved one
    Resolve {
        slug: String,
        id: String,
        #[arg(long, conflicts_with_all = ["remove_worktree", "skip_copy", "discard_uncopied"])]
        reopen: bool,
        #[arg(long)]
        remove_worktree: bool,
        #[arg(long)]
        skip_copy: bool,
        #[arg(long, requires = "remove_worktree")]
        discard_uncopied: bool,
    },
}

/// `-` is standard input; a relative path is relative to the caller's directory.
fn read_text(file: &str) -> Result<String> {
    use std::io::Read;
    if file == "-" {
        let mut text = String::new();
        std::io::stdin().read_to_string(&mut text)?;
        Ok(text)
    } else {
        std::fs::read_to_string(file).map_err(|e| anyhow::anyhow!("could not read {file}: {e}"))
    }
}

#[derive(Subcommand)]
enum RoutineCommand {
    /// Approve a routine's command (a person at a terminal only)
    Approve { slug: String, name: String },
    /// List routines with their approval status
    List { slug: String },
}

#[derive(Subcommand)]
enum SafetyCommand {
    /// Print the effective safety settings and the config.toml table to edit
    Show { slug: String },
}

#[derive(Subcommand)]
enum TickerCommand {
    /// Start the ticker if it is not running (does nothing when there are no projects)
    Start,
    /// Run the ticker loop in the foreground
    Run,
    /// Ask the running ticker to exit and wait for it
    Stop,
    /// Show the running ticker's version, root and tool resolution
    Status,
}

pub fn run() -> Result<()> {
    let cli = Cli::parse();
    let env = Env::from_process()?;
    let config_dir = env.config_dir();
    let root = paths::resolve_root(cli.root.as_deref(), &env, &config_dir)?;
    let runner = RealRunner;
    let ctx = Ctx {
        env: &env,
        root,
        config_dir,
        runner: &runner,
        detached_ticker: true,
    };

    match cli.command {
        Command::New { name, goal, repos } => {
            let repos = repos
                .iter()
                .map(|arg| project::parse_repo_arg(arg))
                .collect();
            let project = project::create(&ctx.root, &name, &goal, repos)?;
            println!("created `{}` at {}", project.slug, project.dir().display());
            println!(
                "next: {} open {}",
                coordinator::current_prefix(&ctx.root)?,
                project.slug
            );
            Ok(())
        }
        Command::List { all } => {
            for slug in project::list_slugs(&ctx.root) {
                let project = Project::load(&ctx.root, &slug)?;
                let status = project.status();
                if status == Status::Archived && !all {
                    continue;
                }
                let mut counts = std::collections::BTreeMap::new();
                for row in threads::rows(&ctx, &project) {
                    *counts
                        .entry(row.group.rank())
                        .or_insert((row.group.label(), 0)) = (
                        row.group.label(),
                        counts
                            .get(&row.group.rank())
                            .map_or(0, |c: &(&str, usize)| c.1)
                            + 1,
                    );
                }
                let summary: Vec<String> = counts
                    .values()
                    .map(|(label, n)| format!("{label}: {n}"))
                    .collect();
                println!(
                    "{slug}\t{status}\t{}",
                    if summary.is_empty() {
                        "no threads".to_string()
                    } else {
                        summary.join(", ")
                    }
                );
            }
            Ok(())
        }
        Command::Open {
            slug,
            reprime,
            rebind,
            session,
        } => coordinator::open(
            &ctx,
            &slug,
            &OpenOptions {
                session: session.into(),
                reprime,
                rebind,
            },
        ),
        Command::Context { slug, peek } => coordinator::context(&ctx, &slug, peek),
        Command::Overview { slug, wait } => overview::run(&ctx, slug.as_deref(), wait),
        Command::Focus { slug } => overview::focus(&ctx, slug.as_deref()),
        Command::Unfocus { session } => overview::unfocus(&ctx, &session.into()),
        Command::Inbox { command } => match command {
            InboxCommand::Done { slug, ids, all } => {
                let project = Project::load(&ctx.root, &slug)?;
                let moved = inbox::done(&project, &ids, all)?;
                println!("{moved} item(s) moved to inbox/done");
                Ok(())
            }
        },
        Command::Thread { command } => match command {
            ThreadCommand::Start {
                slug,
                title,
                repo,
                machine,
                agent,
                base,
                task_file,
            } => {
                let task = read_text(&task_file)?;
                let thread = threads::start(
                    &ctx,
                    &slug,
                    StartArgs {
                        title,
                        repo,
                        machine,
                        agent,
                        base,
                        task,
                        node: NodeRequest::default(),
                    },
                )?;
                println!(
                    "{}",
                    serde_json::json!({ "id": thread.id, "kind": thread.kind, "branch": thread.branch, "pane_id": thread.pane_id })
                );
                Ok(())
            }
            ThreadCommand::Restart { slug, id } => {
                let thread = threads::restart(&ctx, &slug, &id)?;
                println!(
                    "{} is back in pane {}; the ticker launches its agent",
                    thread.id, thread.pane_id
                );
                Ok(())
            }
            ThreadCommand::Prompt {
                slug,
                id,
                text_file,
            } => {
                let text = read_text(&text_file)?;
                let state = threads::prompt(&ctx, &slug, &id, &text)?;
                println!("sent to {id} (agent was {state})");
                Ok(())
            }
            ThreadCommand::Adopt {
                slug,
                pane,
                title,
                task_file,
            } => {
                let task = task_file.map(|file| read_text(&file)).transpose()?;
                let thread = adopt::adopt(&ctx, &slug, &pane, &title, task)?;
                println!(
                    "{}",
                    serde_json::json!({ "id": thread.id, "kind": thread.kind, "pane_id": thread.pane_id, "prompt_pending": thread.prompt_pending })
                );
                Ok(())
            }
            ThreadCommand::List { slug } => threads::print_list(&ctx, &slug),
            ThreadCommand::Show { slug, id } => threads::print_show(&ctx, &slug, &id),
            ThreadCommand::Ack { slug, id } => threads::ack(&ctx, &slug, &id),
            ThreadCommand::Resolve {
                slug,
                id,
                reopen,
                remove_worktree,
                skip_copy,
                discard_uncopied,
            } => threads::resolve(
                &ctx,
                &slug,
                &id,
                &ResolveArgs {
                    reopen,
                    remove_worktree,
                    skip_copy,
                    discard_uncopied,
                },
            ),
        },
        Command::Node { command } => match command {
            NodeCommand::Start(args) => {
                let NodeStartArgs {
                    slug,
                    title,
                    parent,
                    role,
                    can_spawn,
                    no_spawn,
                    harness,
                    model,
                    reasoning_effort,
                    permission_profile,
                    raw_agent_args,
                    repo,
                    machine,
                    base,
                    task_file,
                } = *args;
                let task = read_text(&task_file)?;
                let role = role.into();
                let can_spawn = if can_spawn {
                    Some(true)
                } else if no_spawn {
                    Some(false)
                } else {
                    None
                };
                let node = NodeRequest {
                    parent_id: parent,
                    role,
                    can_spawn,
                    profile: ProfileOverrides {
                        harness,
                        model,
                        reasoning_effort,
                        permission_profile,
                        raw_agent_args,
                    },
                };
                let node = threads::start(
                    &ctx,
                    &slug,
                    StartArgs {
                        title,
                        repo,
                        machine,
                        agent: None,
                        base,
                        task,
                        node,
                    },
                )?;
                println!(
                    "{}",
                    serde_json::json!({
                        "id": node.id,
                        "kind": node.kind,
                        "branch": node.branch,
                        "pane_id": node.pane_id,
                        "parent_id": node.parent_id,
                        "role": node.role,
                        "can_spawn": node.can_spawn,
                        "harness": node.agent,
                        "model": node.model,
                        "reasoning_effort": node.reasoning_effort,
                        "permission_profile": node.permission_profile,
                    })
                );
                Ok(())
            }
            NodeCommand::Restart { slug, id } => {
                let node = threads::restart(&ctx, &slug, &id)?;
                println!(
                    "{} is back in pane {}; the ticker launches its agent",
                    node.id, node.pane_id
                );
                Ok(())
            }
            NodeCommand::Prompt {
                slug,
                id,
                text_file,
            } => {
                let text = read_text(&text_file)?;
                let state = threads::prompt(&ctx, &slug, &id, &text)?;
                println!("sent to {id} (agent was {state})");
                Ok(())
            }
            NodeCommand::List { slug } => threads::print_node_list(&ctx, &slug),
            NodeCommand::Show { slug, id } => threads::print_show(&ctx, &slug, &id),
            NodeCommand::Ack { slug, id } => threads::ack(&ctx, &slug, &id),
            NodeCommand::Resolve {
                slug,
                id,
                reopen,
                remove_worktree,
                skip_copy,
                discard_uncopied,
            } => threads::resolve(
                &ctx,
                &slug,
                &id,
                &ResolveArgs {
                    reopen,
                    remove_worktree,
                    skip_copy,
                    discard_uncopied,
                },
            ),
        },
        Command::Routine { command } => match command {
            RoutineCommand::Approve { slug, name } => {
                let project = Project::load(&ctx.root, &slug)?;
                routine::approve(&ctx.config_dir, &project, &name)
            }
            RoutineCommand::List { slug } => {
                let project = Project::load(&ctx.root, &slug)?;
                let commands = project.safety(&ctx.config_dir)?.routine_commands;
                routine::print_list(&ctx.config_dir, &project, commands);
                Ok(())
            }
        },
        Command::Pause { slug } => lifecycle::set_status(&ctx, &slug, Status::Paused),
        Command::Resume { slug } => {
            if Project::load(&ctx.root, &slug)?.status() == Status::Archived {
                bail!("`{slug}` is archived; use `unarchive`");
            }
            lifecycle::set_status(&ctx, &slug, Status::Active)
        }
        Command::Archive { slug } => lifecycle::set_status(&ctx, &slug, Status::Archived),
        Command::Unarchive { slug } => lifecycle::set_status(&ctx, &slug, Status::Active),
        Command::Delete { slug, force } => lifecycle::delete(&ctx, &slug, force),
        Command::AdoptWorkspace {
            name,
            goal,
            pane,
            workspace_cwd,
            session,
        } => adopt::adopt_workspace(
            &ctx,
            &adopt::AdoptWorkspace {
                name,
                goal,
                pane,
                workspace_cwd,
                session: session.into(),
            },
        ),
        Command::Action { id } => actions::run_action(&ctx, &id),
        Command::Pane { id } => actions::run_pane(&ctx, &id),
        Command::Safety { command } => match command {
            SafetyCommand::Show { slug } => {
                let project = Project::load(&ctx.root, &slug)?;
                let safety = project.safety(&ctx.config_dir)?;
                println!("Effective safety settings for `{slug}`:");
                println!("  start_threads = {:?}", safety.start_threads);
                println!(
                    "  coordinator_agent_args = {:?}",
                    safety.coordinator_agent_args
                );
                println!("  thread_agent_args = {:?}", safety.thread_agent_args);
                println!("  routine_commands = {}", safety.routine_commands);
                println!();
                println!(
                    "To change one, edit {} by hand and add:",
                    ctx.config_dir.join("config.toml").display()
                );
                println!();
                println!("[safety.{:?}]", project.canonical_dir().to_string_lossy());
                Ok(())
            }
        },
        Command::Skill => {
            print!("{}", include_str!("../skill/COORDINATOR.md"));
            Ok(())
        }
        Command::Doctor { session } => {
            if !doctor::run(&ctx, &session.into())? {
                bail!("some checks failed");
            }
            Ok(())
        }
        Command::Ticker { command } => match command {
            TickerCommand::Start => ticker::start(&ctx),
            TickerCommand::Run => ticker::run(&ctx),
            TickerCommand::Stop => ticker::stop(&ctx.root),
            TickerCommand::Status => ticker::status(&ctx.root),
        },
    }
}

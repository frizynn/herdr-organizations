use std::io::Write as _;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};

use crate::agent_profile::ProfileOverrides;
use crate::coordinator::{self, OpenOptions};
use crate::organizations::{self, NodeRequest};
use crate::paths::{self, Ctx, Env, SessionFlags};
use crate::project::{self, Project, Status};
use crate::runner::RealRunner;
use crate::thread::{self, NodeRole};
use crate::threads::{self, ResolveArgs, StartArgs};
use crate::{actions, adopt, doctor, inbox, lifecycle, overview, routine, templates, ticker};

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
    /// Reusable node templates: role, agent profile, rules and memory
    Template {
        #[command(subcommand)]
        command: TemplateCommand,
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
    /// Run inside a plugin popup or split pane
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
    /// Print and archive one bounded batch of new events
    Consume { slug: String },
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
        /// Close the recorded Herdr surface, keeping Git artifacts
        #[arg(long, conflicts_with_all = ["reopen", "remove_worktree"])]
        close_view: bool,
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
    #[arg(long, value_enum)]
    role: Option<CliNodeRole>,
    #[arg(long, value_name = "NAME")]
    template: Option<String>,
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
    #[arg(long, value_name = "FILE")]
    rules_file: Option<String>,
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
    /// Changes apply to the next brief or restart of this node and its descendants, not to an ongoing conversation
    Rules {
        slug: String,
        id: String,
        #[arg(long, value_name = "FILE")]
        text_file: Option<String>,
    },
    /// Print a child's short summary from its report
    Summary { slug: String, id: String },
    /// Record that the user has seen the current report
    Ack { slug: String, id: String },
    /// Resolve a node, or reopen a resolved one
    Resolve {
        slug: String,
        id: String,
        #[arg(long, conflicts_with_all = ["remove_worktree", "skip_copy", "discard_uncopied"])]
        reopen: bool,
        /// Close the recorded Herdr surface, keeping Git artifacts
        #[arg(long, conflicts_with_all = ["reopen", "remove_worktree"])]
        close_view: bool,
        #[arg(long)]
        remove_worktree: bool,
        #[arg(long)]
        skip_copy: bool,
        #[arg(long, requires = "remove_worktree")]
        discard_uncopied: bool,
    },
}

#[derive(Subcommand)]
enum TemplateCommand {
    /// List templates
    List {
        #[arg(long)]
        project: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Show one template
    Show {
        name: String,
        #[arg(long)]
        project: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Save or replace a template
    Save {
        name: String,
        #[arg(long)]
        project: Option<String>,
        #[arg(long, value_name = "SLUG ID", num_args = 2)]
        from_node: Option<Vec<String>>,
        #[arg(long)]
        description: Option<String>,
        #[arg(long, value_enum)]
        role: Option<CliNodeRole>,
        #[arg(long, conflicts_with = "no_spawn")]
        can_spawn: bool,
        #[arg(long = "no-spawn", conflicts_with = "can_spawn")]
        no_spawn: bool,
        #[arg(long)]
        harness: Option<String>,
        #[arg(long)]
        model: Option<String>,
        #[arg(long)]
        reasoning_effort: Option<String>,
        #[arg(long)]
        permission_profile: Option<String>,
        #[arg(long = "raw-agent-arg")]
        raw_agent_args: Vec<String>,
        #[arg(long, value_name = "FILE")]
        rules_file: Option<String>,
        #[arg(long)]
        force: bool,
    },
    /// Read or replace a template's memory
    Memory {
        name: String,
        #[arg(long)]
        project: Option<String>,
        #[arg(long, value_name = "FILE")]
        text_file: Option<String>,
    },
    /// Delete a template
    Delete {
        name: String,
        #[arg(long)]
        project: Option<String>,
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

fn template_json(template: &templates::Template) -> Result<serde_json::Value> {
    Ok(serde_json::json!({
        "name": template.spec.name,
        "scope": match template.scope {
            templates::Scope::Global => "global",
            templates::Scope::Project => "project",
        },
        "project": template.project,
        "description": template.spec.description,
        "role": template.spec.role,
        "can_spawn": template.spec.can_spawn,
        "harness": template.spec.harness,
        "model": template.spec.model,
        "reasoning_effort": template.spec.reasoning_effort,
        "permission_profile": template.spec.permission_profile,
        "rules_chars": templates::rules(template)?.chars().count(),
        "memory_chars": templates::memory_body(&templates::memory(template)?).chars().count(),
        "updated": template.spec.updated,
        "dir": template.dir.to_string_lossy(),
    }))
}

fn node_request_from_template(
    parent_id: String,
    role: Option<NodeRole>,
    can_spawn: bool,
    no_spawn: bool,
    overrides: ProfileOverrides,
    spec: &templates::TemplateSpec,
) -> NodeRequest {
    let role = role.unwrap_or(spec.role);
    let can_spawn = if can_spawn {
        Some(true)
    } else if no_spawn {
        Some(false)
    } else if role == spec.role {
        Some(spec.can_spawn)
    } else {
        None
    };
    NodeRequest {
        parent_id,
        role,
        can_spawn,
        profile: ProfileOverrides {
            harness: template_value(overrides.harness, &spec.harness),
            model: template_value(overrides.model, &spec.model),
            reasoning_effort: template_value(overrides.reasoning_effort, &spec.reasoning_effort),
            permission_profile: template_value(
                overrides.permission_profile,
                &spec.permission_profile,
            ),
            raw_agent_args: if overrides.raw_agent_args.is_empty() {
                spec.raw_agent_args.clone()
            } else {
                overrides.raw_agent_args
            },
        },
    }
}

fn template_value(flag: Option<String>, spec: &str) -> Option<String> {
    flag.or_else(|| (!spec.is_empty()).then(|| spec.to_string()))
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
            InboxCommand::Consume { slug } => {
                let project = Project::load(&ctx.root, &slug)?;
                let stdout = std::io::stdout();
                let mut out = stdout.lock();
                inbox::consume(&project, &mut out)?;
                out.flush()?;
                Ok(())
            }
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
                        rules: String::new(),
                        node: NodeRequest::default(),
                        template: String::new(),
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
                close_view,
                remove_worktree,
                skip_copy,
                discard_uncopied,
            } => threads::resolve(
                &ctx,
                &slug,
                &id,
                &ResolveArgs {
                    reopen,
                    close_view,
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
                    rules_file,
                    template,
                } = *args;
                let overrides = ProfileOverrides {
                    harness,
                    model,
                    reasoning_effort,
                    permission_profile,
                    raw_agent_args,
                };
                let (rules, node, template_name) = match template {
                    Some(name) => {
                        let template = templates::resolve(&ctx.root, Some(&slug), &name)?;
                        if rules_file.is_some() {
                            bail!("pass --rules-file or --template, not both");
                        }
                        let rules = templates::rules(&template)?;
                        let node = node_request_from_template(
                            parent,
                            role.map(Into::into),
                            can_spawn,
                            no_spawn,
                            overrides,
                            &template.spec,
                        );
                        (rules, node, template.spec.name.clone())
                    }
                    None => {
                        let rules = match rules_file {
                            Some(f) if f == "-" => {
                                bail!("--rules-file does not read standard input; --task-file may")
                            }
                            Some(f) => read_text(&f)?,
                            None => String::new(),
                        };
                        let role = role.map(Into::into).unwrap_or(NodeRole::Worker);
                        let can_spawn = if can_spawn {
                            Some(true)
                        } else if no_spawn {
                            Some(false)
                        } else {
                            None
                        };
                        (
                            rules,
                            NodeRequest {
                                parent_id: parent,
                                role,
                                can_spawn,
                                profile: overrides,
                            },
                            String::new(),
                        )
                    }
                };
                let task = read_text(&task_file)?;
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
                        rules,
                        node,
                        template: template_name,
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
                        "template": node.template,
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
            NodeCommand::Rules {
                slug,
                id,
                text_file,
            } => {
                let project = Project::load(&ctx.root, &slug)?;
                if let Some(text_file) = text_file {
                    let rules = read_text(&text_file)?;
                    if rules.len() > organizations::MAX_NODE_RULES_BYTES {
                        bail!(
                            "--rules-file is over {} bytes",
                            organizations::MAX_NODE_RULES_BYTES
                        );
                    }
                    let _lock = project.lock()?;
                    thread::load(&project, &id)?;
                    let instructions =
                        organizations::node_scope_dir(&project, &id).join("INSTRUCTIONS.md");
                    project::write_atomic(
                        &instructions,
                        organizations::node_instructions(&rules).as_bytes(),
                    )?;
                } else {
                    thread::load(&project, &id)?;
                    let instructions =
                        organizations::node_scope_dir(&project, &id).join("INSTRUCTIONS.md");
                    print!("{}", std::fs::read_to_string(instructions)?);
                }
                Ok(())
            }
            NodeCommand::Summary { slug, id } => {
                let project = Project::load(&ctx.root, &slug)?;
                thread::load(&project, &id)?;
                let report_path = thread::home_report_path(&project, &id);
                let report = match std::fs::read_to_string(&report_path) {
                    Ok(report) => report,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        println!("no report yet");
                        return Ok(());
                    }
                    Err(error) => {
                        eprintln!(
                            "could not read report at {}: {error}",
                            report_path.display()
                        );
                        return Ok(());
                    }
                };
                if let Some(summary) = organizations::report_summary(&report) {
                    println!("{summary}");
                } else {
                    println!(
                        "no ## Summary in report ({} characters at {}); ask the node to add one",
                        report.chars().count(),
                        report_path.display()
                    );
                }
                Ok(())
            }
            NodeCommand::Ack { slug, id } => threads::ack(&ctx, &slug, &id),
            NodeCommand::Resolve {
                slug,
                id,
                reopen,
                close_view,
                remove_worktree,
                skip_copy,
                discard_uncopied,
            } => threads::resolve(
                &ctx,
                &slug,
                &id,
                &ResolveArgs {
                    reopen,
                    close_view,
                    remove_worktree,
                    skip_copy,
                    discard_uncopied,
                },
            ),
        },
        Command::Template { command } => match command {
            TemplateCommand::List { project, json } => {
                let templates = templates::list(&ctx.root, project.as_deref())?;
                if json {
                    let rows = templates
                        .iter()
                        .map(template_json)
                        .collect::<Result<Vec<_>>>()?;
                    println!("{}", serde_json::to_string(&rows)?);
                } else {
                    for template in templates {
                        println!(
                            "{}\t{}\t{}\t{}",
                            template.spec.name,
                            match template.scope {
                                templates::Scope::Global => "global",
                                templates::Scope::Project => "project",
                            },
                            template.spec.role.as_str(),
                            template.spec.description
                        );
                    }
                }
                Ok(())
            }
            TemplateCommand::Show {
                name,
                project,
                json,
            } => {
                let template = templates::resolve(&ctx.root, project.as_deref(), &name)?;
                let rules = templates::rules(&template)?;
                let memory = templates::memory(&template)?;
                if json {
                    let mut object = template_json(&template)?
                        .as_object()
                        .cloned()
                        .context("template JSON did not serialize as an object")?;
                    object.insert("rules".into(), serde_json::json!(rules));
                    object.insert("memory".into(), serde_json::json!(memory));
                    println!("{}", serde_json::Value::Object(object));
                } else {
                    print!("{}", toml::to_string(&template.spec)?);
                    println!("## Rules\n{rules}\n## Memory\n{memory}");
                }
                Ok(())
            }
            TemplateCommand::Save {
                name,
                project,
                from_node,
                description,
                role,
                can_spawn,
                no_spawn,
                harness,
                model,
                reasoning_effort,
                permission_profile,
                raw_agent_args,
                rules_file,
                force,
            } => {
                let (mut spec, mut rules) = match from_node.as_deref() {
                    Some([slug, id]) => templates::spec_from_node(
                        &ctx.root,
                        slug,
                        id,
                        &name,
                        description.as_deref().unwrap_or(""),
                    )?,
                    Some(_) => bail!("--from-node requires a project slug and node id"),
                    None => {
                        let role = role.ok_or_else(|| {
                            anyhow::anyhow!("--role is required unless --from-node is used")
                        })?;
                        (
                            templates::TemplateSpec {
                                role: role.into(),
                                ..templates::TemplateSpec::default()
                            },
                            String::new(),
                        )
                    }
                };
                if let Some(description) = description {
                    spec.description = description;
                }
                if let Some(role) = role {
                    spec.role = role.into();
                }
                if let Some(harness) = harness {
                    spec.harness = harness;
                }
                if let Some(model) = model {
                    spec.model = model;
                }
                if let Some(reasoning_effort) = reasoning_effort {
                    spec.reasoning_effort = reasoning_effort;
                }
                if let Some(permission_profile) = permission_profile {
                    spec.permission_profile = permission_profile;
                }
                if !raw_agent_args.is_empty() {
                    spec.raw_agent_args = raw_agent_args;
                }
                spec.can_spawn = if can_spawn {
                    true
                } else if no_spawn {
                    false
                } else {
                    spec.role == NodeRole::Coordinator
                };
                if let Some(rules_file) = rules_file {
                    if rules_file == "-" {
                        bail!("--rules-file does not read standard input");
                    }
                    rules = read_text(&rules_file)?;
                }
                let template = templates::save(
                    &ctx.root,
                    templates::SaveArgs {
                        name,
                        project,
                        spec,
                        rules,
                        force,
                    },
                )?;
                println!("{}", template_json(&template)?);
                Ok(())
            }
            TemplateCommand::Memory {
                name,
                project,
                text_file,
            } => {
                let template = templates::resolve(&ctx.root, project.as_deref(), &name)?;
                if let Some(text_file) = text_file {
                    let text = read_text(&text_file)?;
                    templates::set_memory(&template, &text)?;
                } else {
                    print!("{}", templates::memory(&template)?);
                }
                Ok(())
            }
            TemplateCommand::Delete { name, project } => {
                templates::delete(&ctx.root, project.as_deref(), &name)?;
                Ok(())
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_flags_override_profile_and_empty_spec_values_inherit() {
        let spec = templates::TemplateSpec {
            role: NodeRole::Coordinator,
            can_spawn: true,
            harness: "codex".into(),
            model: String::new(),
            reasoning_effort: "high".into(),
            permission_profile: "workspace-write".into(),
            raw_agent_args: vec!["--from-template".into()],
            ..templates::TemplateSpec::default()
        };
        let request = node_request_from_template(
            "root".into(),
            None,
            false,
            false,
            ProfileOverrides {
                harness: Some("claude".into()),
                model: Some("gpt-test".into()),
                reasoning_effort: Some("low".into()),
                permission_profile: Some("read-only".into()),
                raw_agent_args: vec!["--from-flag".into()],
            },
            &spec,
        );

        assert_eq!(request.role, NodeRole::Coordinator);
        assert_eq!(request.can_spawn, Some(true));
        assert_eq!(request.profile.harness.as_deref(), Some("claude"));
        assert_eq!(request.profile.model.as_deref(), Some("gpt-test"));
        assert_eq!(request.profile.reasoning_effort.as_deref(), Some("low"));
        assert_eq!(
            request.profile.permission_profile.as_deref(),
            Some("read-only")
        );
        assert_eq!(request.profile.raw_agent_args, ["--from-flag"]);

        let inherited_spec = node_request_from_template(
            "root".into(),
            None,
            false,
            false,
            ProfileOverrides::default(),
            &spec,
        );
        assert_eq!(inherited_spec.profile.harness.as_deref(), Some("codex"));
        assert_eq!(inherited_spec.profile.model, None);
        assert_eq!(
            inherited_spec.profile.reasoning_effort.as_deref(),
            Some("high")
        );
        assert_eq!(
            inherited_spec.profile.permission_profile.as_deref(),
            Some("workspace-write")
        );
        assert_eq!(inherited_spec.profile.raw_agent_args, ["--from-template"]);

        let mut leaf_spec = spec.clone();
        leaf_spec.can_spawn = false;
        let can_spawn = node_request_from_template(
            "root".into(),
            None,
            true,
            false,
            ProfileOverrides::default(),
            &leaf_spec,
        );
        assert_eq!(can_spawn.can_spawn, Some(true));
        let no_spawn = node_request_from_template(
            "root".into(),
            None,
            false,
            true,
            ProfileOverrides::default(),
            &spec,
        );
        assert_eq!(no_spawn.can_spawn, Some(false));

        let empty_spec = templates::TemplateSpec::default();
        let inherited = node_request_from_template(
            "root".into(),
            None,
            false,
            false,
            ProfileOverrides::default(),
            &empty_spec,
        );
        assert_eq!(inherited.profile.harness, None);
        assert_eq!(inherited.profile.model, None);
        assert_eq!(inherited.profile.reasoning_effort, None);
        assert_eq!(inherited.profile.permission_profile, None);
        assert!(inherited.profile.raw_agent_args.is_empty());
    }

    #[test]
    fn template_spawn_policy_is_unset_when_role_changes() {
        let spec = templates::TemplateSpec {
            role: NodeRole::Coordinator,
            can_spawn: true,
            ..templates::TemplateSpec::default()
        };
        let request = node_request_from_template(
            "root".into(),
            Some(NodeRole::Worker),
            false,
            false,
            ProfileOverrides::default(),
            &spec,
        );

        assert_eq!(request.role, NodeRole::Worker);
        assert_eq!(request.can_spawn, None);
    }
}

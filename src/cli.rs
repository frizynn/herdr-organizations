use std::io::IsTerminal as _;
use std::io::Write as _;
use std::path::PathBuf;

use anyhow::{Result, bail};
use clap::{Args, Parser, Subcommand};

use crate::agent_profile::ProfileOverrides;
use crate::coordinator::{self, OpenOptions};
use crate::organizations::{self, NodeRequest};
use crate::paths::{self, Ctx, Env, SessionFlags};
use crate::project::{self, Project, Status};
use crate::runner::RealRunner;
use crate::thread::{self, NodeRole};
use crate::threads::{self, ResolveArgs, StartArgs};
use crate::{
    actions, adopt, doctor, inbox, lifecycle, overview, pr, pr_actions, routine, steps, ticker,
};

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

/// The node flags `open` and `coordinator replace` take on top of a profile,
/// as `node start` does: `--harness` is the profile of that name.
#[derive(Args, Clone, Default)]
pub struct LaunchArgs {
    /// A Herdr agent kind (claude, codex, ...): the profile of that name, one the project allows
    #[arg(long, value_name = "HARNESS", conflicts_with = "profile")]
    harness: Option<String>,
    /// The model, passed as the harness's --model; replaces the profile's
    #[arg(long, value_name = "NAME")]
    model: Option<String>,
    /// The reasoning effort, as the harness's own flag; replaces the profile's (`profile list` shows the levels)
    #[arg(long, value_name = "LEVEL")]
    reasoning_effort: Option<String>,
}

impl From<LaunchArgs> for ProfileOverrides {
    fn from(args: LaunchArgs) -> Self {
        ProfileOverrides {
            harness: args.harness,
            model: args.model,
            reasoning_effort: args.reasoning_effort,
            ..ProfileOverrides::default()
        }
    }
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
        /// Print the versioned JSON contract (docs/json.md)
        #[arg(long)]
        json: bool,
        /// The project's default thread profile (default: thread_profile in [defaults] of config.toml, else claude)
        #[arg(long, value_name = "NAME")]
        thread_profile: Option<String>,
        /// The project's default coordinator profile (default: coordinator_profile in [defaults], else claude)
        #[arg(long, value_name = "NAME")]
        coordinator_profile: Option<String>,
    },
    /// List projects
    List {
        /// Include archived projects
        #[arg(long)]
        all: bool,
        /// Print the versioned JSON contract (docs/json.md)
        #[arg(long)]
        json: bool,
    },
    /// Open a project: start a coordinator agent in its folder, in this pane when
    /// run from a shell pane inside Herdr, else in the project's workspace
    Open {
        slug: String,
        /// The coordinator's profile, one the project allows (default: coordinator_profile in PROJECT.md); `profile list` shows them
        #[arg(long, alias = "agent", value_name = "NAME")]
        profile: Option<String>,
        #[command(flatten)]
        launch: LaunchArgs,
        /// Start another coordinator even though one is running
        #[arg(long)]
        new: bool,
        /// Start the coordinator in a new tab even when run from a shell pane
        /// inside Herdr (by default it starts in that pane)
        #[arg(long)]
        tab: bool,
        /// Move the project to this session when its recorded socket no longer exists
        #[arg(long)]
        rebind: bool,
        #[command(flatten)]
        session: SessionArgs,
    },
    /// The coordinator: send it a sentence
    Coordinator {
        #[command(subcommand)]
        command: CoordinatorCommand,
    },
    /// Print the digest the coordinator reads at the start of every turn
    Context {
        slug: String,
        /// Print without recording the inbox items as seen
        #[arg(long)]
        peek: bool,
    },
    /// Who TASKS.md tasks may be assigned to: this project's thread profiles, and other machines with their profiles
    Assignable {
        slug: String,
        /// Look up other machines' profiles again instead of using the hour-long cache
        #[arg(long)]
        refresh: bool,
        /// Exit with an error unless OWNER (me, PROFILE, @MACHINE or PROFILE@MACHINE) is valid
        #[arg(long, value_name = "OWNER")]
        check: Option<String>,
    },
    /// Print threads grouped by what needs you
    Overview {
        slug: Option<String>,
        /// Wait for Enter before exiting (only when on a terminal; used by the popup)
        #[arg(long, conflicts_with = "json")]
        wait: bool,
        /// Print the versioned JSON contract (docs/json.md); never asks for a project
        #[arg(long)]
        json: bool,
    },
    /// List what you asked every coordinator, with the state it wrote and the one its links confirm
    Asks {
        /// Only this project's coordinators; every project that is not archived by default
        #[arg(long, value_name = "SLUG")]
        project: Option<String>,
        /// Print the versioned JSON contract (docs/json.md)
        #[arg(long)]
        json: bool,
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
    /// Node templates: a node's role, profile, rules and memory, to start other nodes from
    Template {
        #[command(subcommand)]
        command: TemplateCommand,
    },
    /// Routines: scheduled prompts and watched commands
    Routine {
        #[command(subcommand)]
        command: RoutineCommand,
    },
    /// The project's settings and what every thread receives from it
    Project {
        #[command(subcommand)]
        command: ProjectCommand,
    },
    /// The tasks: TASKS.md, which only the coordinator writes, or the tracker config.toml names
    Task {
        #[command(subcommand)]
        command: TaskCommand,
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
    /// Give a project a new slug (its folder name), and with --name a new display name.
    /// Refused while a thread is not resolved. While agents run in the project folder
    /// (its coordinator too) the ticker does it once they are idle: it closes their
    /// panes, renames, and reopens the coordinator in the new folder
    Rename {
        slug: String,
        new_slug: String,
        /// The new display name (default: keep it)
        #[arg(long, value_name = "NAME")]
        name: Option<String>,
        /// Print the plan and change nothing
        #[arg(long)]
        dry_run: bool,
        #[command(flatten)]
        session: SessionArgs,
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
    /// Run by the popup as it closes: waits for the popup's process to exit
    /// (the end of standard input), then jumps
    #[command(hide = true)]
    Jump {
        #[command(subcommand)]
        jump: crate::ui_ops::Jump,
    },
    /// Agent profiles: named launch setups (harness, model, effort, flags) and which ones threads and coordinators may use
    Profile {
        #[command(subcommand)]
        command: ProfileCommand,
    },
    /// Safety settings
    Safety {
        #[command(subcommand)]
        command: SafetyCommand,
    },
    /// Print the coordinator skill
    Skill,
    /// Check the setup: versions, tools, root, ticker and each project's files and session
    Doctor {
        /// Repair what can be repaired: priming files, uploads/, stale binary paths
        #[arg(long)]
        fix: bool,
        #[command(flatten)]
        session: SessionArgs,
    },
    /// List what a project left behind (orphan worktrees, merged branches, tabs, old folders) and remove it
    Sweep {
        slug: String,
        /// Only list
        #[arg(long)]
        dry_run: bool,
        /// Remove without asking
        #[arg(long)]
        yes: bool,
    },
    /// Change one setting in PROJECT.md (name, goal, icon, tone, coordinator_profile, thread_profile, max_parallel_threads, auto_resolve_days, nudge, mute, progress, turn_reminder, records, repos.add, repos.remove)
    Set {
        slug: String,
        key: String,
        value: String,
    },
    /// Open a file: text in a new Herdr tab running $EDITOR, anything else with the system opener
    OpenFile {
        path: PathBuf,
        /// The workspace to add the tab to (default: the current one)
        #[arg(long, value_name = "ID")]
        workspace: Option<String>,
    },
    /// Open a URL (a pull request) in the browser
    OpenUrl { url: String },
    /// The projects popup, in this terminal
    Popup {
        /// Scope it to one project (default: the current workspace's, else all)
        slug: Option<String>,
    },
    /// Install the plugin's progress hooks (Claude Code, Codex, Droid, Gemini CLI, Copilot CLI) and its `autoproject` skill (Claude Code, Codex)
    Configure {
        /// Harnesses to configure, comma-separated: claude, codex, droid, gemini, copilot (default: those installed)
        #[arg(long, value_delimiter = ',', value_parser = crate::setup::AGENTS)]
        clients: Vec<String>,
        #[arg(long, value_name = "DIR")]
        claude_home: Option<PathBuf>,
        #[arg(long, value_name = "DIR")]
        codex_home: Option<PathBuf>,
        /// Print what would change and change nothing
        #[arg(long)]
        dry_run: bool,
        /// The key that opens the Organizations launcher (default: prefix+a)
        #[arg(long, value_name = "KEY")]
        key: Option<String>,
        /// Only the hooks: leave Herdr's config.toml alone
        #[arg(long)]
        hooks_only: bool,
    },
    /// Print `projects: N need you` for the tab bar (nothing when none, or when the ticker is not running)
    NeedsYou {
        #[arg(long)]
        line: bool,
    },
    /// Run by Herdr at startup: the ticker and the default sidebar order
    #[command(hide = true)]
    Startup,
    /// Remove exactly what `configure` added
    Unconfigure,
    /// Report your progress (run by an agent in its own Herdr pane)
    Report {
        #[arg(long, value_parser = clap::value_parser!(u8).range(0..=100), required_unless_present = "unknown", conflicts_with = "unknown")]
        percent: Option<u8>,
        #[arg(long)]
        unknown: bool,
        #[arg(long)]
        activity: String,
    },
    /// Harness hook entry point (installed by `configure`)
    #[command(hide = true)]
    Hook {
        #[arg(long, value_parser = crate::setup::AGENTS)]
        agent: String,
    },
    /// Print the progress record of this pane, or of --pane
    Progress {
        #[arg(long, value_name = "ID")]
        pane: Option<String>,
    },
    /// Update the plugin to the newest release: fetch, rebuild, `doctor --fix`, restart the ticker
    Update {
        /// Print the installed and the newest version and change nothing
        #[arg(long)]
        check: bool,
    },
    /// The background ticker
    Ticker {
        #[command(subcommand)]
        command: TickerCommand,
    },
    /// Herdr's own sidebar layout for the organization tokens
    Sidebar {
        #[command(subcommand)]
        command: SidebarCommand,
    },
}

#[derive(Subcommand)]
enum SidebarCommand {
    /// Append the rows that show `$org_need`, `$org_work`, `$org_review` and
    /// `$org_task` to Herdr's config.toml (a dated backup is kept)
    Install {
        /// Herdr config file (default: $HERDR_CONFIG_PATH, then ~/.config/herdr/config.toml)
        #[arg(long, value_name = "PATH")]
        config: Option<PathBuf>,
        /// Print the edited file instead of writing it
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Subcommand)]
enum InboxCommand {
    /// List unhandled items without archiving or marking them seen
    List {
        slug: String,
        /// Print the versioned JSON contract (docs/json.md)
        #[arg(long)]
        json: bool,
    },
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
enum CoordinatorCommand {
    /// Send a sentence to the project's coordinator (the popup's task keys use this)
    Prompt {
        slug: String,
        /// The text; `-` reads standard input
        #[arg(long, value_name = "FILE")]
        text_file: String,
    },
    /// Stop the project's running coordinator and start a new one on --profile or --harness
    #[command(group(clap::ArgGroup::new("choice").required(true).args(["profile", "harness"])))]
    Replace {
        slug: String,
        /// The profile the new coordinator runs on (claude, codex, or one of yours)
        #[arg(long)]
        profile: Option<String>,
        #[command(flatten)]
        launch: LaunchArgs,
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
        /// The agent's profile (harness, model, effort, flags), one the project allows (default: thread_profile in PROJECT.md); `context` lists them
        #[arg(long, alias = "agent", value_name = "NAME")]
        profile: Option<String>,
        /// Placement, not the agent: worktree (default with --repo), tab (default without), or checkout (a tab on the repo's main checkout)
        #[arg(long, value_name = "worktree|tab|checkout")]
        kind: Option<String>,
        #[arg(long, value_name = "REF")]
        base: Option<String>,
        /// The task; `-` reads standard input
        #[arg(long, value_name = "FILE")]
        task_file: String,
        /// Delegating this task (its title as written in TASKS.md, or a tracker task's id or title): its notes are added to the task, its owner gives --profile and --machine, and the task line gets the thread's id (a tracker's `link` is told instead)
        #[arg(long, value_name = "TITLE")]
        from_task: Option<String>,
    },
    /// Bring back a thread whose pane is gone or whose start failed
    Restart {
        slug: String,
        id: String,
        /// Restart with another profile the project allows (default: the thread's own)
        #[arg(long, alias = "agent", value_name = "NAME")]
        profile: Option<String>,
    },
    /// Send a follow-up to a thread's agent (recorded in its task file)
    Prompt {
        slug: String,
        id: String,
        /// The text; `-` reads standard input
        #[arg(long, value_name = "FILE")]
        text_file: String,
    },
    /// A thread's Next list: print it, forward line N as a prompt, or add a line
    Next {
        slug: String,
        id: String,
        /// Forward this line (1-based) to the thread as a prompt
        #[arg(long, value_name = "N", conflicts_with = "add")]
        line: Option<usize>,
        /// Add a line to the list
        #[arg(long, value_name = "TEXT")]
        add: Option<String>,
    },
    /// Send Escape to a thread's pane (the harness's own interrupt)
    Stop { slug: String, id: String },
    /// Deliver a thread's brief now, when its agent is ready but the ticker has not sent it yet
    Brief { slug: String, id: String },
    /// Print what a thread's pane shows now (a trust dialog, a question menu, a permission prompt)
    Read {
        slug: String,
        id: String,
        /// Read the last N lines of scrollback instead of the visible screen
        #[arg(long, value_name = "N")]
        lines: Option<usize>,
    },
    /// Answer what a thread's pane shows: type --text (no Enter), then press the keys (enter, esc, up, down, tab, 1, y, ctrl+c)
    Keys {
        slug: String,
        id: String,
        #[arg(value_name = "KEY")]
        keys: Vec<String>,
        /// Literal text typed before the keys
        #[arg(long, value_name = "TEXT", allow_hyphen_values = true)]
        text: Option<String>,
    },
    /// List threads with live state and group
    List {
        slug: String,
        /// Print the versioned JSON contract (docs/json.md)
        #[arg(long)]
        json: bool,
    },
    /// Show one thread's record, group, note and Next list
    Show {
        slug: String,
        id: String,
        /// Print the versioned JSON contract (docs/json.md)
        #[arg(long)]
        json: bool,
    },
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
    /// Merge the thread's pull request: only when it is open, approved and every check passed, or its project's merge gate passes
    Merge {
        slug: String,
        id: String,
        /// Default: the project's `merge_method` in config.toml, else squash
        #[arg(long, value_enum)]
        method: Option<pr::MergeMethod>,
        /// Print the versioned JSON contract (docs/json.md)
        #[arg(long)]
        json: bool,
    },
    /// Ask a thread to fix CI, address review comments or resolve merge conflicts on its pull request: a follow-up with counts and `gh` commands read from GitHub now, refused when the pull request does not call for it
    PrAction {
        slug: String,
        id: String,
        #[arg(value_enum)]
        action: pr_actions::Request,
        /// Print the versioned JSON contract (docs/json.md)
        #[arg(long)]
        json: bool,
    },
    /// Open the pull request of a thread whose agent is idle and whose branch is pushed without one: its title, and its report as the description
    PrCreate {
        slug: String,
        id: String,
        /// Print the versioned JSON contract (docs/json.md)
        #[arg(long)]
        json: bool,
    },
    /// Turn the per-thread pull request automation on or off
    #[command(group = clap::ArgGroup::new("flag").required(true).multiple(true))]
    Set {
        slug: String,
        id: String,
        /// Prompt the agent when its pull request has failing checks or review comments
        #[arg(long, value_enum, group = "flag")]
        auto_fix_ci: Option<Toggle>,
        /// Merge the pull request once it is approved and every check passed
        #[arg(long, value_enum, group = "flag")]
        auto_merge: Option<Toggle>,
        /// Print the versioned JSON contract (docs/json.md)
        #[arg(long)]
        json: bool,
    },
    /// Resolve a thread: final copy home, then its worktree, merged branch and tab are cleaned up (reports and library are kept)
    Resolve {
        slug: String,
        id: String,
        #[arg(long, conflicts_with_all = ["keep_worktree", "skip_copy", "discard_uncopied"])]
        reopen: bool,
        /// Close the recorded Herdr surface, keeping Git artifacts (worktree and branch)
        #[arg(long, conflicts_with = "reopen")]
        close_view: bool,
        /// Before 0.2.20 a resolve kept the worktree unless this was given; it is now the default
        #[arg(long, hide = true, conflicts_with_all = ["keep_worktree", "close_view"])]
        remove_worktree: bool,
        /// Keep the worktree and branch
        #[arg(long)]
        keep_worktree: bool,
        /// Resolve even though the final copy cannot be made (the worktree is then kept)
        #[arg(long)]
        skip_copy: bool,
        /// Remove the worktree even though not everything in it was copied home
        #[arg(long)]
        discard_uncopied: bool,
    },
}

#[derive(clap::ValueEnum, Clone, Copy)]
enum Toggle {
    On,
    Off,
}

impl Toggle {
    fn on(self) -> bool {
        matches!(self, Toggle::On)
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
    /// Coordinators can create children; workers cannot (default: the template's, else worker)
    #[arg(long, value_enum)]
    role: Option<NodeRole>,
    /// Start from a template: its role, profile, rules and memory; the flags given here win
    #[arg(long, value_name = "NAME")]
    template: Option<String>,
    /// Explicitly grant a coordinator permission to create children
    #[arg(long, conflicts_with = "no_spawn")]
    can_spawn: bool,
    /// Create a leaf coordinator that cannot create children
    #[arg(long = "no-spawn", conflicts_with = "can_spawn")]
    no_spawn: bool,
    /// The node's profile, one the project allows (default: its parent's, else the project's default for the role)
    #[arg(long, value_name = "NAME", conflicts_with = "harness")]
    profile: Option<String>,
    /// A Herdr agent kind, the built-in profile of that name
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
    /// Placement: worktree (default with --repo), tab (default without), or checkout
    #[arg(long, value_name = "worktree|tab|checkout")]
    kind: Option<String>,
    #[arg(long, value_name = "REF")]
    base: Option<String>,
    /// The task; `-` reads standard input
    #[arg(long, value_name = "FILE")]
    task_file: String,
    /// Standing instructions for the node and its descendants, kept in its INSTRUCTIONS.md (up to 8 KiB); they replace a template's
    #[arg(long, value_name = "FILE")]
    rules_file: Option<String>,
    /// Delegating this task (its title as written in TASKS.md, or a tracker task's id or title): its notes are added to the task, its owner gives --profile and --machine, and the task line gets the node's id (a tracker's `link` is told instead)
    #[arg(long, value_name = "TITLE")]
    from_task: Option<String>,
}

#[derive(Subcommand)]
enum TemplateCommand {
    /// Save a node as a template: its role, profile name, rules and memory
    Save {
        slug: String,
        id: String,
        #[arg(long)]
        name: String,
        /// Keep it in the config folder for every project, not in this project
        #[arg(long)]
        global: bool,
        #[arg(long, default_value = "")]
        description: String,
        /// Replace a template of that name
        #[arg(long)]
        force: bool,
    },
    /// List the global templates, and with --project that project's own over them
    List {
        #[arg(long, value_name = "SLUG")]
        project: Option<String>,
        /// Print the JSON contract (docs/json.md)
        #[arg(long)]
        json: bool,
    },
    /// Show a template with its rules and memory: the project's own, else the global one
    Show {
        name: String,
        #[arg(long, value_name = "SLUG")]
        project: Option<String>,
        /// Print the JSON contract (docs/json.md)
        #[arg(long)]
        json: bool,
    },
    /// Delete a project's template, or a global one without --project
    Delete {
        name: String,
        #[arg(long, value_name = "SLUG")]
        project: Option<String>,
    },
}

#[derive(Subcommand)]
enum NodeCommand {
    /// Create and start a child node; `create` is an alias
    #[command(alias = "create")]
    Start(Box<NodeStartArgs>),
    /// Bring back a node whose pane is gone or whose start failed
    Restart {
        slug: String,
        id: String,
        /// Restart with another profile the project allows (default: the node's own)
        #[arg(long, value_name = "NAME")]
        profile: Option<String>,
    },
    /// Send a follow-up to a node's agent
    Prompt {
        slug: String,
        id: String,
        #[arg(long, value_name = "FILE")]
        text_file: String,
    },
    /// List nodes with live state, role and parent
    List {
        slug: String,
        /// Print the versioned JSON contract (docs/json.md)
        #[arg(long)]
        json: bool,
    },
    /// Show one node's record
    Show {
        slug: String,
        id: String,
        /// Print the versioned JSON contract (docs/json.md)
        #[arg(long)]
        json: bool,
    },
    /// Token use per node, read from its harness's transcripts now: context, peak, compactions and totals
    Usage {
        slug: String,
        /// Print the versioned JSON contract (docs/json.md)
        #[arg(long)]
        json: bool,
    },
    /// Print a node's standing instructions, or replace them; the node and its descendants read them on their next brief or restart
    Rules {
        slug: String,
        id: String,
        #[arg(long, value_name = "FILE")]
        text_file: Option<String>,
    },
    /// Record that the user has seen the current report
    Ack { slug: String, id: String },
    /// Resolve a node, or reopen a resolved one
    Resolve {
        slug: String,
        id: String,
        #[arg(long, conflicts_with_all = ["keep_worktree", "close_view", "skip_copy", "discard_uncopied"])]
        reopen: bool,
        /// Close the recorded Herdr surface, keeping Git artifacts (worktree and branch)
        #[arg(long)]
        close_view: bool,
        /// Keep the worktree and branch
        #[arg(long)]
        keep_worktree: bool,
        /// Before 0.2.20 a resolve kept the worktree unless this was given; it is now the default
        #[arg(long, hide = true, conflicts_with_all = ["keep_worktree", "close_view"])]
        remove_worktree: bool,
        #[arg(long)]
        skip_copy: bool,
        /// Remove the worktree even though not everything in it was copied home
        #[arg(long)]
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
    /// Enable or disable a routine
    Toggle {
        slug: String,
        name: String,
        #[arg(long, conflicts_with = "off")]
        on: bool,
        #[arg(long)]
        off: bool,
    },
    /// Approve a routine's command (a person at a terminal only)
    Approve { slug: String, name: String },
    /// List routines with their approval status
    List {
        slug: String,
        /// Print the versioned JSON contract (docs/json.md)
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum ProjectCommand {
    /// Settings, repos, instructions and memory
    Show {
        slug: String,
        /// Print the versioned JSON contract (docs/json.md)
        #[arg(long)]
        json: bool,
    },
    /// Tasks, threads, pull requests and drift, computed from the files on every call
    Status {
        slug: String,
        /// Print the versioned JSON contract (docs/json.md)
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum TaskCommand {
    /// List the tasks; ask the coordinator to change them (`coordinator prompt`)
    List {
        slug: String,
        /// Print the versioned JSON contract (docs/json.md)
        #[arg(long)]
        json: bool,
    },
    /// Where the tasks disagree with the thread records; it changes nothing
    Lint {
        slug: String,
        /// Print the versioned JSON contract (docs/json.md)
        #[arg(long)]
        json: bool,
    },
}

/// The typed fields of a profile, for `add` and `edit`.
#[derive(Args)]
struct ProfileFields {
    /// Model name, passed as --model (empty: the harness's default)
    #[arg(long, value_name = "NAME")]
    model: Option<String>,
    /// Reasoning effort: claude low..max, codex none..ultra, copilot none..max, pi/omp off..max (others: put it in --arg or the model id)
    #[arg(long, value_name = "LEVEL")]
    effort: Option<String>,
    /// One line on when to use it; the coordinator reads it when choosing
    #[arg(long, value_name = "TEXT")]
    description: Option<String>,
    /// An extra argument for the agent CLI, repeatable (--arg --config --arg ~/.omp/agent/luna.yml)
    #[arg(long = "arg", value_name = "ARG", allow_hyphen_values = true)]
    args: Vec<String>,
}

#[derive(Subcommand)]
enum ProfileCommand {
    /// List the profiles (yours and the installed, signed-in harnesses), and with --project what it allows
    List {
        #[arg(long, value_name = "SLUG")]
        project: Option<String>,
        /// Only the names, one per line (what other machines read to assign tasks here)
        #[arg(long, conflicts_with = "project")]
        names: bool,
        /// With --project: what its coordinator and threads may run, as the versioned JSON contract (docs/json.md)
        #[arg(long, requires = "project")]
        json: bool,
    },
    /// Print a profile's launch setup here as JSON (default: [defaults] thread_profile); other machines read it to start threads here
    Resolve { name: Option<String> },
    /// Add a profile (a person at a terminal only)
    Add {
        name: String,
        /// The harness: a Herdr agent kind (claude, codex, cursor, gemini, opencode, copilot, omp, pi, ...)
        #[arg(long, value_name = "KIND")]
        agent: String,
        #[command(flatten)]
        fields: ProfileFields,
    },
    /// Change a profile's fields; --arg replaces its arguments (a person at a terminal only)
    Edit {
        name: String,
        #[arg(long, value_name = "KIND")]
        agent: Option<String>,
        #[command(flatten)]
        fields: ProfileFields,
        /// Remove all its extra arguments
        #[arg(long, conflicts_with = "args")]
        clear_args: bool,
    },
    /// Remove a profile (a person at a terminal only)
    Remove { name: String },
    /// Set which profiles threads or coordinators may use, for one project or as the default for all (a person at a terminal only)
    Allow {
        #[arg(value_parser = ["threads", "coordinator"])]
        role: String,
        #[arg(required_unless_present = "all")]
        names: Vec<String>,
        /// Only this project (default: every project without its own list)
        #[arg(long, value_name = "SLUG")]
        project: Option<String>,
        /// Allow every profile (removes the list)
        #[arg(long, conflicts_with = "names")]
        all: bool,
    },
    /// The profile new projects start with for threads or coordinators; one project's own is `set <slug> thread_profile NAME` (a person at a terminal only)
    Default {
        #[arg(value_parser = ["threads", "coordinator"])]
        role: String,
        name: String,
    },
}

#[derive(Subcommand)]
enum SafetyCommand {
    /// Print the effective safety settings, where each comes from, and how to change them
    Show {
        /// A project, or --global for the all-projects defaults
        #[arg(allow_hyphen_values = true)]
        target: String,
    },
    /// Turn yolo mode on or off for a project or, with --global, for all projects (a person at a terminal only)
    Yolo {
        /// A project, or --global for the all-projects default
        #[arg(allow_hyphen_values = true)]
        target: String,
        /// on, off, or default (use the all-projects value)
        #[arg(value_parser = ["on", "off", "default"])]
        state: String,
    },
    /// Change one safety setting for a project or, with --global, for all projects (a person at a terminal only)
    Set {
        /// A project, or --global for the all-projects defaults
        #[arg(allow_hyphen_values = true)]
        target: String,
        /// yolo, start_threads, trust_screens, coordinator_agent_args, thread_agent_args or routine_commands
        key: String,
        /// The value (arguments for *_agent_args, none for an empty list), or `default`
        #[arg(allow_hyphen_values = true, trailing_var_arg = true)]
        value: Vec<String>,
    },
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

fn profile_change(ctx: &Ctx, command: ProfileCommand) -> Result<crate::profiles::Change> {
    use crate::profiles::{Change, Entry, Role};
    Ok(match command {
        ProfileCommand::List { .. } | ProfileCommand::Resolve { .. } => {
            bail!("`profile list` and `profile resolve` change nothing")
        }
        ProfileCommand::Add {
            name,
            agent,
            fields,
        } => Change::Add {
            name,
            entry: Entry {
                agent,
                model: fields.model.unwrap_or_default(),
                effort: fields.effort.unwrap_or_default(),
                args: fields.args,
                description: fields.description.unwrap_or_default(),
            },
        },
        ProfileCommand::Edit {
            name,
            agent,
            fields,
            clear_args,
        } => Change::Edit {
            name,
            agent,
            model: fields.model,
            effort: fields.effort,
            description: fields.description,
            args: if clear_args {
                Some(Vec::new())
            } else {
                (!fields.args.is_empty()).then_some(fields.args)
            },
        },
        ProfileCommand::Remove { name } => Change::Remove { name },
        ProfileCommand::Allow {
            role,
            names,
            project,
            all,
        } => Change::Allow {
            role: Role::parse(&role)?,
            project: project
                .map(|slug| Project::load(&ctx.root, &slug).map(|p| p.canonical_dir()))
                .transpose()?,
            names: (!all).then_some(names),
        },
        ProfileCommand::Default { role, name } => Change::Default {
            role: Role::parse(&role)?,
            name,
        },
    })
}

/// A `profile ...` command line the popup builds, applied in this process:
/// the popup is a person's own screen, so it needs no terminal check.
pub fn apply_profile_args(ctx: &Ctx, args: &[String]) -> Result<String> {
    #[derive(Parser)]
    #[command(name = "profile")]
    struct ProfileCli {
        #[command(subcommand)]
        command: ProfileCommand,
    }
    let parsed = ProfileCli::try_parse_from(args).map_err(|e| {
        anyhow::anyhow!(
            "{}",
            e.to_string().lines().next().unwrap_or("bad arguments")
        )
    })?;
    let change = profile_change(ctx, parsed.command)?;
    crate::profiles::apply(&ctx.config_dir, &change)
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
        Command::New {
            name,
            goal,
            repos,
            json,
            thread_profile,
            coordinator_profile,
        } => {
            let repos = repos
                .iter()
                .map(|arg| project::parse_repo_arg(arg))
                .collect();
            let config = crate::profiles::load(&ctx.config_dir)?;
            let thread = thread_profile
                .unwrap_or_else(|| config.new_project_default(crate::profiles::Role::Thread));
            let coordinator = coordinator_profile
                .unwrap_or_else(|| config.new_project_default(crate::profiles::Role::Coordinator));
            for name in [&thread, &coordinator] {
                if config.get(name).is_none() {
                    bail!("there is no profile `{name}`; `profile list` shows them");
                }
            }
            let project = project::create_for(&ctx.config_dir, &ctx.root, &name, &goal, repos)?;
            crate::profiles::write_project_defaults(&project, &thread, &coordinator)?;
            let prefix = coordinator::current_prefix(&ctx.root)?;
            project::write_priming(&project, &prefix)?;
            let next = format!("{prefix} open {}", project.slug);
            if json {
                return threads::print_json(&serde_json::json!({
                    "schema_version": threads::JSON_SCHEMA_VERSION,
                    "project": overview::project_json(&ctx, &project, &[]),
                    "next": next,
                }));
            }
            println!("created `{}` at {}", project.slug, project.dir().display());
            println!("next: {next}");
            Ok(())
        }
        Command::List { all, json } => {
            let mut projects = Vec::new();
            for slug in project::list_slugs(&ctx.root) {
                let project = Project::load(&ctx.root, &slug)?;
                let status = project.status();
                if status == Status::Archived && !all {
                    continue;
                }
                let rows = threads::rows(&ctx, &project);
                if json {
                    projects.push(overview::project_json(&ctx, &project, &rows));
                    continue;
                }
                let summary: Vec<String> = overview::group_counts(&rows)
                    .into_iter()
                    .map(|(group, n)| format!("{}: {n}", group.label()))
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
            if json {
                threads::print_json(&serde_json::json!({
                    "schema_version": threads::JSON_SCHEMA_VERSION,
                    "projects": projects,
                }))?;
            }
            Ok(())
        }
        Command::Open {
            slug,
            profile,
            launch,
            new,
            tab,
            rebind,
            session,
        } => coordinator::open(
            &ctx,
            &slug,
            &OpenOptions {
                session: session.into(),
                rebind,
                profile,
                launch: launch.into(),
                new,
                // Only a person at a terminal gets the agent in place; the
                // popup and agents' shell tools run `open` without one.
                here: !tab && std::io::stdin().is_terminal() && std::io::stdout().is_terminal(),
            },
        ),
        Command::Coordinator { command } => match command {
            CoordinatorCommand::Prompt { slug, text_file } => {
                let text = organizations::signed(&ctx, &slug, &read_text(&text_file)?)?;
                coordinator::prompt(&ctx, &slug, &text)
            }
            CoordinatorCommand::Replace {
                slug,
                profile,
                launch,
            } => {
                println!(
                    "{}",
                    crate::ui_ops::replace_coordinator(
                        &ctx,
                        &slug,
                        profile.as_deref().unwrap_or(""),
                        &launch.into()
                    )?
                );
                Ok(())
            }
        },
        Command::Context { slug, peek } => coordinator::context(&ctx, &slug, peek),
        Command::Assignable {
            slug,
            refresh,
            check,
        } => crate::assign::run(&ctx, &slug, refresh, check.as_deref()),
        Command::Asks { project, json } => crate::asks::print(&ctx, project.as_deref(), json),
        Command::Overview { slug, wait, json } => {
            if json {
                overview::run_json(&ctx, slug.as_deref())
            } else {
                overview::run(&ctx, slug.as_deref(), wait)
            }
        }
        Command::Focus { slug } => overview::focus(&ctx, slug.as_deref()),
        Command::Unfocus { session } => overview::unfocus(&ctx, &session.into()),
        Command::Inbox { command } => match command {
            InboxCommand::List { slug, json } => {
                inbox::print_list(&Project::load(&ctx.root, &slug)?, json)
            }
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
                profile,
                kind,
                base,
                task_file,
                from_task,
            } => {
                let task = read_text(&task_file)?;
                let kind = kind
                    .as_deref()
                    .map(crate::thread::Kind::parse)
                    .transpose()?;
                let thread = threads::start(
                    &ctx,
                    &slug,
                    StartArgs {
                        title,
                        repo,
                        machine,
                        profile,
                        kind,
                        base,
                        task,
                        node: NodeRequest {
                            from_task,
                            ..NodeRequest::default()
                        },
                    },
                )?;
                println!(
                    "{}",
                    serde_json::json!({ "id": thread.id, "kind": thread.kind, "profile": thread.profile, "agent": thread.agent, "branch": thread.branch, "pane_id": thread.pane_id })
                );
                Ok(())
            }
            ThreadCommand::Restart { slug, id, profile } => {
                let thread = threads::restart(&ctx, &slug, &id, profile.as_deref())?;
                println!(
                    "{} is back in pane {}; the ticker launches its {} agent",
                    thread.id,
                    thread.pane_id,
                    if thread.profile.is_empty() {
                        &thread.agent
                    } else {
                        &thread.profile
                    }
                );
                Ok(())
            }
            ThreadCommand::Next {
                slug,
                id,
                line,
                add,
            } => threads::next(&ctx, &slug, &id, line, add.as_deref()),
            ThreadCommand::Stop { slug, id } => threads::stop(&ctx, &slug, &id),
            ThreadCommand::Brief { slug, id } => threads::brief(&ctx, &slug, &id),
            ThreadCommand::Read { slug, id, lines } => threads::read(&ctx, &slug, &id, lines),
            ThreadCommand::Keys {
                slug,
                id,
                keys,
                text,
            } => threads::keys(&ctx, &slug, &id, &keys, text.as_deref()),
            ThreadCommand::Prompt {
                slug,
                id,
                text_file,
            } => {
                let text = organizations::signed(&ctx, &slug, &read_text(&text_file)?)?;
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
            ThreadCommand::List { slug, json } => threads::print_list(&ctx, &slug, json),
            ThreadCommand::Merge {
                slug,
                id,
                method,
                json,
            } => {
                let project = Project::load(&ctx.root, &slug)?;
                threads::refuse_agent_pane(&ctx, &project, "`thread merge`")?;
                let record = thread::load(&project, &id)?;
                let method = match method {
                    Some(method) => method,
                    None => project.safety(&ctx.config_dir)?.merge_method,
                };
                steps::merge_pull_request(&ctx, &project, &id, method)?;
                // The merge already happened; a missing item must not report failure.
                if let Err(error) = inbox::write(
                    &project,
                    "pr",
                    &id,
                    "merged",
                    &format!(
                        "{id} \"{}\": pull request {} was merged with `thread merge`",
                        record.title, record.pr
                    ),
                    "",
                ) {
                    eprintln!("merged, but the inbox item was not written: {error:#}");
                }
                if json {
                    return threads::print_json(&serde_json::json!({
                        "schema_version": threads::JSON_SCHEMA_VERSION,
                        "id": id,
                        "pr": record.pr,
                        "merged": true,
                    }));
                }
                println!("{id}: merged {}", record.pr);
                Ok(())
            }
            ThreadCommand::PrAction {
                slug,
                id,
                action,
                json,
            } => {
                let outcome = pr_actions::ask(&ctx, &slug, &id, action)?;
                if json {
                    threads::print_json(&outcome)?;
                } else if outcome.sent {
                    println!(
                        "sent to {id} (agent was {}):\n{}",
                        outcome.agent_state, outcome.message
                    );
                } else if outcome.ok {
                    println!("{id}: not sent: {}", outcome.reason);
                }
                if !outcome.ok {
                    bail!("{id}: not sent: {}", outcome.reason);
                }
                Ok(())
            }
            ThreadCommand::PrCreate { slug, id, json } => {
                let created = pr_actions::create(&ctx, &slug, &id)?;
                if json {
                    threads::print_json(&created)?;
                } else if created.ok {
                    println!("{id}: opened {}", created.pr);
                }
                if !created.ok {
                    bail!("{id}: no pull request opened: {}", created.reason);
                }
                Ok(())
            }
            ThreadCommand::Set {
                slug,
                id,
                auto_fix_ci,
                auto_merge,
                json,
            } => {
                if auto_merge.is_some_and(Toggle::on) {
                    let project = Project::load(&ctx.root, &slug)?;
                    threads::refuse_agent_pane(&ctx, &project, "turning auto-merge on")?;
                }
                let record = threads::set_flags(
                    &ctx,
                    &slug,
                    &id,
                    auto_fix_ci.map(Toggle::on),
                    auto_merge.map(Toggle::on),
                )?;
                if json {
                    return threads::print_json(&serde_json::json!({
                        "schema_version": threads::JSON_SCHEMA_VERSION,
                        "id": record.id,
                        "auto_fix_ci": record.auto_fix_ci,
                        "auto_merge": record.auto_merge,
                    }));
                }
                let word = |on: bool| if on { "on" } else { "off" };
                println!(
                    "{id}: auto-fix-ci {}, auto-merge {}",
                    word(record.auto_fix_ci),
                    word(record.auto_merge)
                );
                Ok(())
            }
            ThreadCommand::Show { slug, id, json } => threads::print_show(&ctx, &slug, &id, json),
            ThreadCommand::Ack { slug, id } => threads::ack(&ctx, &slug, &id),
            ThreadCommand::Resolve {
                slug,
                id,
                reopen,
                close_view,
                remove_worktree: _,
                keep_worktree,
                skip_copy,
                discard_uncopied,
            } => threads::resolve(
                &ctx,
                &slug,
                &id,
                &ResolveArgs {
                    reopen,
                    keep_worktree,
                    close_view,
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
                    profile,
                    harness,
                    model,
                    reasoning_effort,
                    permission_profile,
                    raw_agent_args,
                    repo,
                    machine,
                    kind,
                    base,
                    task_file,
                    rules_file,
                    template,
                    from_task,
                } = *args;
                let kind = kind
                    .as_deref()
                    .map(crate::thread::Kind::parse)
                    .transpose()?;
                let rules = match rules_file.as_deref() {
                    Some("-") => {
                        bail!("--rules-file does not read standard input; --task-file may")
                    }
                    Some(file) => read_text(file)?,
                    None => String::new(),
                };
                let task = read_text(&task_file)?;
                let can_spawn = if can_spawn {
                    Some(true)
                } else if no_spawn {
                    Some(false)
                } else {
                    None
                };
                let mut start = StartArgs {
                    title,
                    repo,
                    machine,
                    profile,
                    kind,
                    base,
                    task,
                    node: NodeRequest {
                        parent_id: parent,
                        role: role.unwrap_or_default(),
                        can_spawn,
                        profile: ProfileOverrides {
                            harness,
                            model,
                            reasoning_effort,
                            permission_profile,
                            raw_agent_args,
                        },
                        rules,
                        memory: Default::default(),
                        from_task,
                        task_id: String::new(),
                        template_profile: None,
                    },
                };
                if let Some(name) = &template {
                    crate::templates::resolve(&ctx, Some(&slug), name)?.fill(&mut start, role)?;
                }
                let node = threads::start(&ctx, &slug, start)?;
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
                        "profile": node.profile,
                        "harness": node.agent,
                        "model": node.model,
                        "reasoning_effort": node.reasoning_effort,
                        "permission_profile": node.permission_profile,
                        "template": template.unwrap_or_default(),
                    })
                );
                Ok(())
            }
            NodeCommand::Restart { slug, id, profile } => {
                let node = threads::restart(&ctx, &slug, &id, profile.as_deref())?;
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
                let text = organizations::signed(&ctx, &slug, &read_text(&text_file)?)?;
                let state = threads::prompt(&ctx, &slug, &id, &text)?;
                println!("sent to {id} (agent was {state})");
                Ok(())
            }
            NodeCommand::List { slug, json } => {
                if json {
                    threads::print_node_list_json(&ctx, &slug)
                } else {
                    threads::print_node_list(&ctx, &slug)
                }
            }
            NodeCommand::Show { slug, id, json } => threads::print_show(&ctx, &slug, &id, json),
            NodeCommand::Usage { slug, json } => crate::usage::print(&ctx, &slug, json),
            NodeCommand::Rules {
                slug,
                id,
                text_file,
            } => {
                let project = Project::load(&ctx.root, &slug)?;
                match text_file {
                    Some(file) => organizations::set_node_rules(&project, &id, &read_text(&file)?),
                    None => {
                        print!("{}", organizations::node_rules(&project, &id)?);
                        Ok(())
                    }
                }
            }
            NodeCommand::Ack { slug, id } => threads::ack(&ctx, &slug, &id),
            NodeCommand::Resolve {
                slug,
                id,
                reopen,
                close_view,
                keep_worktree,
                remove_worktree: _,
                skip_copy,
                discard_uncopied,
            } => threads::resolve(
                &ctx,
                &slug,
                &id,
                &ResolveArgs {
                    reopen,
                    keep_worktree,
                    close_view,
                    skip_copy,
                    discard_uncopied,
                },
            ),
        },
        Command::Sweep { slug, dry_run, yes } => crate::sweep::run(&ctx, &slug, dry_run, yes),
        Command::Set { slug, key, value } => crate::settings::set(&ctx, &slug, &key, &value),
        Command::OpenFile { path, workspace } => {
            crate::settings::open_file(&ctx, &path, workspace.as_deref())
        }
        Command::OpenUrl { url } => {
            if !url.starts_with("https://") {
                bail!("only https URLs are opened");
            }
            crate::settings::system_open(&ctx, &url)
        }
        Command::Popup { slug } => {
            let scope = match slug {
                Some(slug) => Some(slug),
                None => overview::resolve_slug_quiet(&ctx),
            };
            let workspace = ctx.env.var("HERDR_WORKSPACE_ID").unwrap_or("").to_string();
            crate::popup::run(&ctx, scope, workspace)
        }
        Command::Routine { command } => match command {
            RoutineCommand::Toggle {
                slug,
                name,
                on,
                off,
            } => crate::settings::routine_toggle(
                &ctx,
                &slug,
                &name,
                if on {
                    Some(true)
                } else if off {
                    Some(false)
                } else {
                    None
                },
            ),
            RoutineCommand::Approve { slug, name } => {
                let project = Project::load(&ctx.root, &slug)?;
                routine::approve(&ctx.config_dir, &project, &name)
            }
            RoutineCommand::List { slug, json } => {
                let project = Project::load(&ctx.root, &slug)?;
                let commands = project.safety(&ctx.config_dir)?.routine_commands;
                if json {
                    return threads::print_json(&routine::list_json(
                        &ctx.config_dir,
                        &project,
                        commands,
                    ));
                }
                routine::print_list(&ctx.config_dir, &project, commands);
                Ok(())
            }
        },
        Command::Project {
            command: ProjectCommand::Show { slug, json },
        } => overview::print_project_show(&ctx, &slug, json),
        Command::Project {
            command: ProjectCommand::Status { slug, json },
        } => crate::status::print(&ctx, &slug, json),
        Command::Task {
            command: TaskCommand::List { slug, json },
        } => crate::tasks::print_list(&ctx, &slug, json),
        Command::Task {
            command: TaskCommand::Lint { slug, json },
        } => crate::drift::print_lint(&ctx, &slug, json),
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
        Command::Rename {
            slug,
            new_slug,
            name,
            dry_run,
            session,
        } => crate::rename::cli(
            &ctx,
            &crate::rename::Args {
                from: &slug,
                to: &new_slug,
                name: name.as_deref(),
                dry_run,
                by_ticker: false,
            },
            &session.into(),
        ),
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
        Command::Jump { jump } => {
            jump.after_popup(&ctx);
            Ok(())
        }
        Command::Template { command } => match command {
            TemplateCommand::Save {
                slug,
                id,
                name,
                global,
                description,
                force,
            } => {
                let template =
                    crate::templates::save(&ctx, &slug, &id, &name, global, &description, force)?;
                println!(
                    "saved template `{name}` in {} from {id}",
                    template.dir.display()
                );
                Ok(())
            }
            TemplateCommand::List { project, json } => {
                let templates = crate::templates::list(&ctx, project.as_deref())?;
                if json {
                    let profiles = crate::profiles::load(&ctx.config_dir)?;
                    let rows = templates
                        .iter()
                        .map(|t| t.json(&profiles))
                        .collect::<Result<Vec<_>>>()?;
                    println!("{}", serde_json::Value::Array(rows));
                } else {
                    for template in &templates {
                        println!("{}", template.line());
                    }
                }
                Ok(())
            }
            TemplateCommand::Show {
                name,
                project,
                json,
            } => {
                let template = crate::templates::resolve(&ctx, project.as_deref(), &name)?;
                if json {
                    let profiles = crate::profiles::load(&ctx.config_dir)?;
                    println!("{}", template.show_json(&profiles)?);
                } else {
                    print!("{}", template.show_text()?);
                }
                Ok(())
            }
            TemplateCommand::Delete { name, project } => {
                crate::templates::delete(&ctx, project.as_deref(), &name)?;
                println!("deleted template `{name}`");
                Ok(())
            }
        },
        Command::Profile { command } => {
            if let ProfileCommand::Resolve { name } = &command {
                println!("{}", crate::profiles::resolve_json(&ctx, name.as_deref())?);
                return Ok(());
            }
            if let ProfileCommand::List {
                project,
                names,
                json,
            } = command
            {
                if json {
                    let slug = project.unwrap_or_default();
                    let project = Project::load(&ctx.root, &slug)?;
                    println!("{}", crate::profiles::choices_json(&ctx, &project)?);
                    return Ok(());
                }
                if names {
                    let config = crate::profiles::load(&ctx.config_dir)?;
                    for p in config.listed(&crate::profiles::detect(ctx.env)) {
                        println!("{}", p.name);
                    }
                    return Ok(());
                }
                let project = project
                    .map(|slug| Project::load(&ctx.root, &slug))
                    .transpose()?;
                print!("{}", crate::profiles::list_text(&ctx, project.as_ref())?);
                return Ok(());
            }
            let change = profile_change(&ctx, command)?;
            crate::profiles::require_person()?;
            println!("{}", crate::profiles::apply(&ctx.config_dir, &change)?);
            Ok(())
        }
        Command::Safety { command } => match command {
            SafetyCommand::Show { target } => {
                print!(
                    "{}",
                    crate::safety::show_text(&ctx, &crate::safety::Target::parse(&ctx, &target)?)?
                );
                Ok(())
            }
            SafetyCommand::Yolo { target, state } => {
                crate::safety::set_cli(&ctx, &target, "yolo", &[state])
            }
            SafetyCommand::Set { target, key, value } => {
                crate::safety::set_cli(&ctx, &target, &key, &value)
            }
        },
        Command::Skill => {
            print!("{}", include_str!("../skill/COORDINATOR.md"));
            // Every harness learns to report here; hooks only add reminders.
            // A project with `progress` off gets neither.
            let here = std::env::current_dir().unwrap_or_default();
            if !crate::progress::taught_in(&ctx.root, &here) {
                return Ok(());
            }
            let pane = crate::progress::current(ctx.env, ctx.runner).map(|p| p.pane_id);
            println!(
                "\n## Progress\n\n{}",
                crate::progress::guidance(
                    &crate::coordinator::current_prefix(&ctx.root)?,
                    pane.as_deref()
                )
            );
            Ok(())
        }
        Command::Doctor { fix, session } => {
            if !doctor::run(&ctx, &session.into(), fix)? {
                bail!("some checks failed");
            }
            Ok(())
        }
        Command::Sidebar { command } => match command {
            SidebarCommand::Install { config, dry_run } => {
                let path = config.unwrap_or_else(|| crate::sidebar_config::default_path(&env));
                println!("{}", crate::sidebar_config::install(&path, dry_run)?);
                Ok(())
            }
        },
        Command::Configure {
            clients,
            claude_home,
            codex_home,
            dry_run,
            key,
            hooks_only,
        } => {
            let options = crate::setup::ConfigureOptions {
                clients,
                claude_home,
                codex_home,
                dry_run,
                hooks: true,
                sidebar: !hooks_only,
                key,
                herdr_config: None,
                skill: crate::setup::skill_source(),
            };
            for note in crate::setup::configure(&ctx, &options)? {
                println!("{note}");
            }
            if dry_run {
                println!("dry run: nothing was changed");
                return Ok(());
            }
            println!("configured. `unconfigure` removes exactly these entries.");
            if !hooks_only {
                crate::setup::apply_live(&ctx);
            }
            Ok(())
        }
        Command::NeedsYou { line: _ } => {
            if let Some(line) = crate::sidebar::needs_you_line(&ctx.root) {
                println!("{line}");
            }
            Ok(())
        }
        Command::Startup => {
            // Herdr may reinstall into another folder: keep the command's link current.
            if let Ok(binary) = crate::paths::binary()
                && crate::command_link::installable(&binary)
            {
                let _ = crate::command_link::ensure(ctx.env, &binary);
            }
            ticker::start(&ctx)?;
            crate::setup::apply_view(&ctx);
            Ok(())
        }
        Command::Unconfigure => {
            for note in crate::setup::unconfigure(&ctx)? {
                println!("{note}");
            }
            crate::setup::reload_config(&ctx);
            Ok(())
        }
        Command::Report {
            percent,
            unknown: _,
            activity,
        } => crate::progress::report(&ctx, percent, &activity),
        Command::Hook { agent } => {
            // A hook must never fail the harness: errors are swallowed.
            let _ = crate::progress::hook(&ctx, &agent);
            Ok(())
        }
        Command::Progress { pane } => crate::progress::print(&ctx, pane.as_deref()),
        Command::Update { check } => crate::update::run(&ctx, check),
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
    use crate::ui_ops::Jump;

    #[test]
    fn a_handed_off_jump_parses_back_to_itself() {
        for jump in [
            Jump::Node {
                slug: "acme".into(),
                id: "t-0001".into(),
            },
            Jump::Coordinator {
                slug: "acme".into(),
            },
            Jump::Replace {
                slug: "acme".into(),
                profile: "codex".into(),
            },
            Jump::File {
                path: "/r/acme/nodes/t-0001/STATUS.md".into(),
                workspace: Some("w2".into()),
            },
            Jump::File {
                path: "/r/acme/STATUS.md".into(),
                workspace: None,
            },
        ] {
            let args = [vec!["herdr-organizations"], jump.args()].concat();
            match Cli::try_parse_from(args).unwrap().command {
                Command::Jump { jump: parsed } => assert_eq!(parsed, jump),
                _ => panic!("{jump:?} did not parse as a jump"),
            }
        }
    }
}

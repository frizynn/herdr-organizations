//! `<root>/.organizations-state.json`: the one view model every screen reads.
//!
//! The ticker writes it after each pass (only when something changed); the
//! popup and the dock read and watch it and never poll Herdr. When the ticker
//! is not running, the popup builds the same model from the records alone.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::herdr::{Agent, Pane, TabInfo, WorkspaceInfo};
use crate::organizations::{self, ROOT_ID};
use crate::paths::Ctx;
use crate::project::{self, Project};
use crate::thread::{self, Group, NodeRole, Thread};
use crate::{inbox, pr, steps, threads};

pub const FILE: &str = ".organizations-state.json";
pub const SCHEMA_VERSION: u32 = 1;
const MAX_INBOX_PER_PROJECT: usize = 50;

pub fn path(root: &Path) -> PathBuf {
    root.join(FILE)
}

/// The five statuses every screen uses, same mapping as the Nenu web app.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Default)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Need,
    Work,
    Review,
    Done,
    #[default]
    Idle,
}

impl Status {
    pub fn from_group(group: Group) -> Status {
        match group {
            Group::WaitingOnYou => Status::Need,
            Group::Working => Status::Work,
            Group::ReadyForReview | Group::Landing => Status::Review,
            Group::Resolved => Status::Done,
            Group::Idle => Status::Idle,
        }
    }

    pub fn word(self) -> &'static str {
        match self {
            Status::Need => "needs you",
            Status::Work => "working",
            Status::Review => "review",
            Status::Done => "resolved",
            Status::Idle => "idle",
        }
    }

    pub const ORDER: [Status; 5] = [
        Status::Need,
        Status::Work,
        Status::Review,
        Status::Done,
        Status::Idle,
    ];
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct Counts {
    pub need: usize,
    pub work: usize,
    pub review: usize,
    pub done: usize,
    pub idle: usize,
}

impl Counts {
    pub fn add(&mut self, status: Status) {
        *self.slot(status) += 1;
    }

    fn slot(&mut self, status: Status) -> &mut usize {
        match status {
            Status::Need => &mut self.need,
            Status::Work => &mut self.work,
            Status::Review => &mut self.review,
            Status::Done => &mut self.done,
            Status::Idle => &mut self.idle,
        }
    }

    pub fn get(&self, status: Status) -> usize {
        match status {
            Status::Need => self.need,
            Status::Work => self.work,
            Status::Review => self.review,
            Status::Done => self.done,
            Status::Idle => self.idle,
        }
    }

    pub fn total(&self) -> usize {
        self.need + self.work + self.review + self.done + self.idle
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct Pr {
    pub number: String,
    pub url: String,
    pub state: String,
    pub review: String,
    pub draft: bool,
    pub passed: usize,
    pub pending: usize,
    pub failed: usize,
    pub additions: u64,
    pub deletions: u64,
    /// `None` when `thread merge` would merge it now.
    pub blocker: Option<String>,
}

impl Pr {
    pub fn checks(&self) -> usize {
        self.passed + self.pending + self.failed
    }

    pub fn approved(&self) -> bool {
        self.review.eq_ignore_ascii_case("approved")
    }

    pub fn merged(&self) -> bool {
        self.state.eq_ignore_ascii_case("merged")
    }
}

/// A coordinator (the project root or a coordinator node) or a thread.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct Node {
    /// `root` for the project coordinator.
    pub id: String,
    pub title: String,
    pub role: String,
    /// The nearest coordinator above this node: `root` or a node id.
    pub coordinator: String,
    pub parent: String,
    pub depth: usize,
    pub tree_order: usize,
    /// The finer group token (`ready-for-review`, `landing`, ...).
    pub group: String,
    pub status: Status,
    /// What one line can say: a question, a title, a PR state.
    pub text: String,
    pub agent_state: String,
    pub since: String,
    pub harness: String,
    pub model: String,
    pub effort: String,
    pub workspace_id: String,
    pub workspace: String,
    pub tab_id: String,
    pub tab: String,
    pub pane_id: String,
    pub branch: String,
    pub worktree: String,
    pub machine: String,
    pub resolved: bool,
    pub pr: Option<Pr>,
    /// Subtree counts; only filled for coordinators.
    pub counts: Counts,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct InboxItem {
    pub id: String,
    pub kind: String,
    pub subject: String,
    pub created: String,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct ProjectView {
    pub slug: String,
    pub name: String,
    pub dir: String,
    pub repo: String,
    pub status: String,
    pub socket: String,
    pub thread_harness: String,
    /// The project coordinator first, then coordinator nodes in tree order.
    pub coordinators: Vec<Node>,
    /// Workers in tree order.
    pub threads: Vec<Node>,
    pub counts: Counts,
    pub inbox: Vec<InboxItem>,
    /// A coordinator priming or a thread launch or brief is still due.
    pub pending: bool,
    /// Seconds until a blocked agent counts as "needs you" (the 30 s
    /// debounce), so the ticker can look again exactly then.
    pub recheck_in: Option<u64>,
}

impl ProjectView {
    pub fn node(&self, id: &str) -> Option<&Node> {
        self.coordinators
            .iter()
            .chain(&self.threads)
            .find(|n| n.id == id)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct Workspace {
    pub id: String,
    pub label: String,
    pub agent_status: String,
    pub tabs: usize,
    pub focused: bool,
    /// The project whose coordinator or thread lives here, if any.
    pub project: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct Ticker {
    pub pid: u32,
    pub version: String,
    pub events: bool,
    pub last_event: String,
    pub last_event_at: String,
    pub peak_rss_kb: u64,
    pub herdr: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct Snapshot {
    pub schema_version: u32,
    pub generated: String,
    /// Empty when the popup built this itself.
    pub ticker: Option<Ticker>,
    pub projects: Vec<ProjectView>,
    pub workspaces: Vec<Workspace>,
}

impl Snapshot {
    pub fn counts(&self) -> Counts {
        let mut all = Counts::default();
        for project in &self.projects {
            for status in Status::ORDER {
                *all.slot(status) += project.counts.get(status);
            }
        }
        all
    }

    pub fn project(&self, slug: &str) -> Option<&ProjectView> {
        self.projects.iter().find(|p| p.slug == slug)
    }
}

/// What one Herdr server shows right now. Missing in the popup's fallback.
#[derive(Debug, Clone, Default)]
pub struct Live {
    pub socket: String,
    pub agents: Vec<Agent>,
    pub panes: Vec<Pane>,
    pub workspaces: Vec<WorkspaceInfo>,
    pub tabs: Vec<TabInfo>,
}

impl Live {
    fn workspace_label(&self, id: &str) -> String {
        self.workspaces
            .iter()
            .find(|w| w.workspace_id == id)
            .map(|w| w.label.clone())
            .unwrap_or_default()
    }

    fn tab_label(&self, id: &str) -> String {
        self.tabs
            .iter()
            .find(|t| t.tab_id == id)
            .map(|t| t.label.clone())
            .unwrap_or_default()
    }
}

fn pr_number(url: &str) -> String {
    url.rsplit('/')
        .next()
        .filter(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
        .map(str::to_string)
        .unwrap_or_default()
}

fn pr_view(t: &Thread, summary: Option<&pr::Summary>) -> Option<Pr> {
    if t.pr.is_empty() {
        return None;
    }
    let mut view = Pr {
        number: pr_number(&t.pr),
        url: t.pr.clone(),
        state: t.pr_state.clone(),
        review: t.pr_review.clone(),
        ..Pr::default()
    };
    if let Some(s) = summary {
        view.state = s.state.clone();
        view.review = s.review_decision.clone();
        view.draft = s.is_draft;
        view.passed = s.checks.passed;
        view.pending = s.checks.pending;
        view.failed = s.checks.failed;
        view.additions = s.additions;
        view.deletions = s.deletions;
        view.blocker = threads::merge_blocker(t, s);
    } else {
        view.blocker = Some("the pull request has not been read yet".into());
    }
    Some(view)
}

/// One line about a node: the PR for review and resolved, the agent's own
/// title while it works, the reason it needs you.
fn node_text(
    t: &Thread,
    status: Status,
    pr: Option<&Pr>,
    live: Option<&Agent>,
    note: &str,
) -> String {
    let title = live
        .map(|a| a.terminal_title_stripped.trim())
        .filter(|s| !s.is_empty());
    match (status, pr) {
        (Status::Review, Some(pr)) => {
            let mut parts = vec![format!("PR #{}", pr.number)];
            if pr.approved() {
                parts.push("approved".into());
            }
            if pr.checks() > 0 {
                parts.push(format!("checks {}/{}", pr.passed, pr.checks()));
            }
            parts.join(" · ")
        }
        (Status::Done, Some(pr)) if pr.merged() => format!("PR #{} merged", pr.number),
        (Status::Done, _) => t.resolved_reason.clone(),
        (Status::Need, _) if t.status == thread::Status::Failed => t.error.clone(),
        _ => title
            .map(str::to_string)
            .unwrap_or_else(|| note.to_string()),
    }
}

/// Builds every project's view. `lives` holds one entry per reachable socket.
pub fn build(ctx: &Ctx, lives: &[Live]) -> Snapshot {
    let mut projects = Vec::new();
    let mut owners: BTreeMap<(String, String), String> = BTreeMap::new();
    for slug in project::list_slugs(&ctx.root) {
        let Ok(project) = Project::load(&ctx.root, &slug) else {
            continue;
        };
        if project.status() == project::Status::Archived {
            continue;
        }
        let view = project_view(&project, lives);
        for node in view.coordinators.iter().chain(&view.threads) {
            if !node.workspace_id.is_empty() && node.machine.is_empty() {
                owners.insert(
                    (view.socket.clone(), node.workspace_id.clone()),
                    slug.clone(),
                );
            }
        }
        projects.push(view);
    }
    let workspaces = lives
        .iter()
        .flat_map(|live| {
            live.workspaces.iter().map(|w| Workspace {
                id: w.workspace_id.clone(),
                label: w.label.clone(),
                agent_status: w.agent_status.clone(),
                tabs: w.tab_count,
                focused: w.focused,
                project: owners
                    .get(&(live.socket.clone(), w.workspace_id.clone()))
                    .cloned()
                    .unwrap_or_default(),
            })
        })
        .collect();
    Snapshot {
        schema_version: SCHEMA_VERSION,
        generated: project::now(),
        ticker: None,
        projects,
        workspaces,
    }
}

pub fn project_view(project: &Project, lives: &[Live]) -> ProjectView {
    let settings = project
        .read_project_md()
        .map(|(s, _)| s)
        .unwrap_or_default();
    let record = project.coordinator().unwrap_or_default();
    let live = lives.iter().find(|l| l.socket == record.socket);
    let now = jiff::Timestamp::now();
    let prs = steps::load_state(project).prs;
    let entries = organizations::tree(project).unwrap_or_default();
    let records: BTreeMap<&str, &Thread> = entries
        .iter()
        .map(|e| (e.thread.id.as_str(), &e.thread))
        .collect();

    let coordinator_of = |t: &Thread| -> String {
        let mut parent = organizations::parent_id(t).to_string();
        while parent != ROOT_ID {
            match records.get(parent.as_str()) {
                Some(p) if p.role == NodeRole::Coordinator => return parent,
                Some(p) => parent = organizations::parent_id(p).to_string(),
                None => break,
            }
        }
        ROOT_ID.to_string()
    };

    let root_agent = live.and_then(|l| {
        l.agents
            .iter()
            .find(|a| crate::coordinator::agent_matches(&record, a))
    });
    let mut root = Node {
        id: ROOT_ID.into(),
        title: "Coordinator".into(),
        role: "coordinator".into(),
        coordinator: ROOT_ID.into(),
        parent: String::new(),
        harness: settings.coordinator_agent.clone(),
        workspace_id: record.workspace_id.clone(),
        tab_id: record.tab_id.clone(),
        pane_id: record.pane_id.clone(),
        agent_state: root_agent
            .map(|a| a.agent_status.clone())
            .unwrap_or_default(),
        ..Node::default()
    };
    root.status = match root.agent_state.as_str() {
        "working" => Status::Work,
        "blocked" => Status::Need,
        _ => Status::Idle,
    };
    root.text = root_agent
        .map(|a| a.terminal_title_stripped.clone())
        .unwrap_or_default();
    if let Some(live) = live {
        root.workspace = live.workspace_label(&root.workspace_id);
        root.tab = live.tab_label(&root.tab_id);
    }

    let pending = record.prime_pending
        || entries.iter().any(|e| {
            matches!(
                e.thread.status,
                thread::Status::Open | thread::Status::Starting
            ) && (e.thread.prompt_pending || e.thread.status == thread::Status::Starting)
        });
    let mut coordinators = vec![root];
    let mut workers = Vec::new();
    let mut recheck_in: Option<u64> = None;
    for entry in &entries {
        let t = &entry.thread;
        let local_live = live.filter(|_| !t.is_remote());
        let (group, note, agent) = match local_live {
            Some(l) if t.status != thread::Status::Resolved => {
                let state = thread::live_state(t, &l.agents, &l.panes, now);
                let fresh = Thread {
                    report_hash: thread::local_report_hash(t)
                        .unwrap_or_else(|| t.report_hash.clone()),
                    ..t.clone()
                };
                let note = if !state.pane_exists {
                    "pane closed".to_string()
                } else {
                    state
                        .agent_state
                        .clone()
                        .unwrap_or_else(|| "no agent".into())
                };
                let agent = l.agents.iter().find(|a| thread::agent_matches(t, a));
                let group = thread::group(&fresh, &state, now);
                if state.agent_state.as_deref() == Some("blocked") && group != Group::WaitingOnYou {
                    let left = (thread::BLOCKED_DEBOUNCE_SECS - state.state_secs).max(1) as u64;
                    recheck_in = Some(recheck_in.map_or(left, |r| r.min(left)));
                }
                (group, note, agent)
            }
            _ => (threads::recorded_group(t), t.last_state.clone(), None),
        };
        let status = Status::from_group(group);
        let pr = pr_view(t, prs.get(&t.id));
        let mut node = Node {
            id: t.id.clone(),
            title: t.title.clone(),
            role: t.role.as_str().into(),
            coordinator: coordinator_of(t),
            parent: organizations::parent_id(t).to_string(),
            depth: entry.depth,
            tree_order: entry.tree_order,
            group: group.token().into(),
            status,
            text: node_text(t, status, pr.as_ref(), agent, &note),
            agent_state: agent.map(|a| a.agent_status.clone()).unwrap_or_default(),
            since: t.last_state_change.clone(),
            harness: t.agent.clone(),
            model: t.model.clone(),
            effort: t.reasoning_effort.clone(),
            workspace_id: t.workspace_id.clone(),
            tab_id: t.tab_id.clone(),
            pane_id: t.pane_id.clone(),
            branch: t.branch.clone(),
            worktree: t.worktree_path.clone(),
            machine: t.machine.clone(),
            resolved: t.status == thread::Status::Resolved,
            pr,
            ..Node::default()
        };
        if let Some(live) = local_live {
            node.workspace = live.workspace_label(&node.workspace_id);
            node.tab = live.tab_label(&node.tab_id);
        }
        if t.role == NodeRole::Coordinator {
            coordinators.push(node);
        } else {
            workers.push(node);
        }
    }

    // Only threads are counted. A thread counts for every coordinator above
    // it, so a nested coordinator's numbers are part of its parent's.
    let mut counts = Counts::default();
    let parents: BTreeMap<String, String> = coordinators
        .iter()
        .map(|c| (c.id.clone(), c.coordinator.clone()))
        .collect();
    let rolled: Vec<(String, Status)> = workers
        .iter()
        .map(|n| (n.coordinator.clone(), n.status))
        .collect();
    for (mut owner, status) in rolled {
        counts.add(status);
        loop {
            if let Some(c) = coordinators.iter_mut().find(|c| c.id == owner) {
                c.counts.add(status);
            }
            match parents.get(&owner) {
                Some(next) if owner != ROOT_ID && *next != owner => owner = next.clone(),
                _ => break,
            }
        }
    }

    let inbox = inbox::unhandled(project)
        .into_iter()
        .rev()
        .take(MAX_INBOX_PER_PROJECT)
        .map(|item| InboxItem {
            id: item.id,
            kind: item.kind,
            subject: item.subject,
            created: item.created,
            summary: item.summary,
        })
        .collect();

    ProjectView {
        slug: project.slug.clone(),
        name: project::display_name(&settings.name, &project.slug),
        dir: project.dir().to_string_lossy().into_owned(),
        repo: settings
            .repos
            .first()
            .map(|r| r.path.clone())
            .unwrap_or_default(),
        status: project.status().to_string(),
        socket: record.socket.clone(),
        thread_harness: settings.thread_agent.clone(),
        coordinators,
        threads: workers,
        counts,
        inbox,
        pending,
        recheck_in,
    }
}

/// Writes only when the content (everything but `generated`) changed, so
/// watchers wake for real changes only. Returns whether it wrote.
pub fn write_if_changed(root: &Path, snapshot: &Snapshot) -> Result<bool> {
    let path = path(root);
    if let Some(previous) = read(root) {
        let same = Snapshot {
            generated: previous.generated.clone(),
            ..snapshot.clone()
        };
        if same == previous {
            return Ok(false);
        }
    }
    project::write_atomic(&path, serde_json::to_string(snapshot)?.as_bytes())?;
    Ok(true)
}

pub fn read(root: &Path) -> Option<Snapshot> {
    let text = std::fs::read_to_string(path(root)).ok()?;
    let snapshot: Snapshot = serde_json::from_str(&text).ok()?;
    (snapshot.schema_version == SCHEMA_VERSION).then_some(snapshot)
}

/// Seconds since an RFC 3339 time, for "4m" style ages.
pub fn age(since: &str) -> String {
    let secs = thread::seconds_since(since, jiff::Timestamp::now());
    if since.is_empty() || secs < 0 {
        return String::new();
    }
    match secs {
        0..60 => "now".into(),
        60..3600 => format!("{}m", secs / 60),
        3600..86400 => format!("{}h", secs / 3600),
        _ => format!("{}d", secs / 86400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scenarios::World;

    fn node_record(project: &Project, id: &str, parent: &str, role: NodeRole, group: Group) {
        let t = Thread {
            id: id.into(),
            title: format!("title {id}"),
            parent_id: parent.into(),
            role,
            can_spawn: role == NodeRole::Coordinator,
            status: thread::Status::Open,
            last_group: group.token().into(),
            ..Thread::default()
        };
        std::fs::write(
            thread::record_path(project, id),
            toml::to_string(&t).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn recorded_view_nests_threads_under_their_coordinators_and_rolls_counts_up() {
        let world = World::new();
        let project = world.project("demo", "a.sock");
        node_record(
            &project,
            "t-0001",
            "root",
            NodeRole::Coordinator,
            Group::Working,
        );
        node_record(
            &project,
            "t-0002",
            "t-0001",
            NodeRole::Worker,
            Group::WaitingOnYou,
        );
        node_record(
            &project,
            "t-0003",
            "t-0001",
            NodeRole::Coordinator,
            Group::Idle,
        );
        node_record(
            &project,
            "t-0004",
            "t-0003",
            NodeRole::Worker,
            Group::ReadyForReview,
        );
        node_record(&project, "t-0005", "root", NodeRole::Worker, Group::Working);

        let view = project_view(&project, &[]);
        let ids: Vec<_> = view.coordinators.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["root", "t-0001", "t-0003"]);
        assert_eq!(view.node("t-0002").unwrap().coordinator, "t-0001");
        assert_eq!(view.node("t-0004").unwrap().coordinator, "t-0003");
        assert_eq!(view.node("t-0005").unwrap().coordinator, "root");
        let c1 = view.node("t-0001").unwrap();
        assert_eq!(
            (c1.counts.need, c1.counts.review, c1.counts.idle),
            (1, 1, 0)
        );
        let root = view.node("root").unwrap();
        assert_eq!(root.counts.total(), 3);
        assert_eq!(view.counts.total(), 3);
        assert_eq!(view.counts.need, 1);
    }

    #[test]
    fn state_file_is_rewritten_only_when_content_changes() {
        let dir = tempfile::tempdir().unwrap();
        let mut snapshot = Snapshot {
            schema_version: SCHEMA_VERSION,
            generated: "2026-10-10T10:00:00Z".into(),
            ..Snapshot::default()
        };
        assert!(write_if_changed(dir.path(), &snapshot).unwrap());
        snapshot.generated = "2026-10-10T10:00:15Z".into();
        assert!(!write_if_changed(dir.path(), &snapshot).unwrap());
        snapshot.workspaces.push(Workspace {
            id: "w1".into(),
            ..Workspace::default()
        });
        assert!(write_if_changed(dir.path(), &snapshot).unwrap());
        assert_eq!(read(dir.path()).unwrap().workspaces.len(), 1);
    }

    #[test]
    fn review_text_names_the_pull_request_and_its_checks() {
        let t = Thread {
            pr: "https://github.com/o/r/pull/1342".into(),
            status: thread::Status::Open,
            ..Thread::default()
        };
        let summary = pr::Summary {
            state: "OPEN".into(),
            review_decision: "APPROVED".into(),
            checks: pr::Checks {
                passed: 5,
                pending: 0,
                failed: 0,
            },
            ..pr::Summary::default()
        };
        let pr = pr_view(&t, Some(&summary)).unwrap();
        assert_eq!(
            node_text(&t, Status::Review, Some(&pr), None, ""),
            "PR #1342 · approved · checks 5/5"
        );
    }
}

//! Tasks per coordinator. Each coordinator owns a task list: the root its
//! `TASKS.md`, a coordinator node `nodes/<id>/TASKS.md`, or with a tracker
//! the list `list` prints for the coordinator's task scope (the root's is
//! the whole project). Each task has zero or more assignees: the nodes whose
//! record names it (`Thread::assigned`) and the thread on its line. A task
//! whose assignee is a coordinator node is delegated: the owner sees it with
//! that node and the node's own assignees nested under it, and the node sees
//! it among its own. The core never reads a task scope; the tracker does.
//!
//! Two views read the same lists: the project's (`task list <slug>`, every
//! owner's tasks, and the lists whose coordinator is gone as unowned) and a
//! coordinator's (`task list <slug> --node <id>`).

use std::collections::HashSet;

use anyhow::{Result, bail};
use serde::Serialize;
use serde_json::Value;

use crate::organizations::ROOT_ID;
use crate::paths::Ctx;
use crate::project::Project;
use crate::tasks::{self, Task, Tasks};
use crate::thread::{self, NodeRole, Status, Thread};
use crate::tracker::{self, Scope};
use crate::{report, threads};

/// Whether a view may run the tracker's commands (within their cache) or
/// reads only what is cached, as the popup and `overview` do.
#[derive(Clone, Copy, PartialEq)]
pub enum Read {
    Run,
    Cached,
}

/// One coordinator's list.
pub struct OwnerList {
    /// `root` or the node's id.
    pub id: String,
    /// The node's title, empty for the root.
    pub title: String,
    /// Its tasks, or why there are none (the tracker is unavailable).
    pub tasks: std::result::Result<Vec<Task>, String>,
}

/// Every list of a project.
pub struct Lists {
    /// The root's whole list, as `task list` printed it before owners: in
    /// `owners`, the root keeps only what no scoped coordinator lists.
    pub project: Tasks,
    pub owners: Vec<OwnerList>,
    /// Lists whose coordinator is resolved or is no coordinator: (its node,
    /// the task).
    pub unowned: Vec<(String, Task)>,
    /// Tasks found only by their id (`get`), with no owner here.
    pub found: Vec<Task>,
    /// Some node is assigned to a task with `--task` or `node assign`.
    assigned: bool,
}

impl Lists {
    /// Whether the project uses more than the root's list: a coordinator
    /// node's own tasks (or its tracker list failing), an unowned list or an
    /// assignment. Without any, every view prints what it printed before.
    pub fn grouped(&self) -> bool {
        self.owners
            .iter()
            .skip(1)
            .any(|o| o.tasks.as_ref().map_or(true, |list| !list.is_empty()))
            || !self.unowned.is_empty()
            || self.assigned
    }

    /// Every task with its owner, each once, the owners' first.
    fn catalog(&self) -> Vec<(&str, &Task)> {
        let mut all: Vec<(&str, &Task)> = Vec::new();
        for owner in &self.owners {
            for task in owner.tasks.as_deref().unwrap_or_default() {
                all.push((owner.id.as_str(), task));
            }
        }
        for (node, task) in &self.unowned {
            all.push((node.as_str(), task));
        }
        for task in &self.found {
            if !all.iter().any(|(_, t)| same(t, task)) {
                all.push(("", task));
            }
        }
        all
    }
}

/// The same task: by tracker id, else by TASKS.md title.
fn same(a: &Task, b: &Task) -> bool {
    if a.id.is_empty() || b.id.is_empty() {
        a.id == b.id && a.title.eq_ignore_ascii_case(&b.title)
    } else {
        a.id == b.id
    }
}

/// Whether `key`, as a record keeps it, names `task`: a tracker task by its
/// id, a TASKS.md task by its title in any case.
fn names(task: &Task, key: &str) -> bool {
    if task.id.is_empty() {
        task.title.eq_ignore_ascii_case(key.trim())
    } else {
        task.id == key.trim()
    }
}

/// A coordinator node's own `TASKS.md`, beside its instructions and memory.
pub fn node_tasks_path(project: &Project, id: &str) -> std::path::PathBuf {
    crate::organizations::node_scope_dir(project, id).join("TASKS.md")
}

/// The nodes that own a list: open coordinator nodes, in tree order.
fn owner_nodes(project: &Project) -> Vec<Thread> {
    crate::organizations::tree(project)
        .unwrap_or_default()
        .into_iter()
        .map(|entry| entry.thread)
        .filter(|t| t.role == NodeRole::Coordinator && t.status != Status::Resolved)
        .collect()
}

/// Every list of `project`.
pub fn lists(ctx: &Ctx, project: &Project, read: Read) -> Lists {
    let nodes = owner_nodes(project);
    let config = project.tracker(&ctx.config_dir);
    let project_tasks = match (&config, read) {
        (Ok(None), _) => Tasks::File(tasks::parse(&tasks::read(&project.dir()))),
        (Ok(Some(config)), Read::Run) => match tracker::tasks(ctx, project, config, false) {
            Ok(list) => Tasks::Tracker(list),
            Err(reason) => Tasks::Unavailable(reason),
        },
        (Ok(Some(config)), Read::Cached) => match tracker::cached(project, config) {
            Some(Ok(list)) => Tasks::Tracker(list),
            Some(Err(reason)) => Tasks::Unavailable(reason),
            None => Tasks::Tracker(Vec::new()),
        },
        (Err(error), _) => Tasks::Unavailable(tracker::config_error(error)),
    };
    let node_list = |t: &Thread| -> std::result::Result<Vec<Task>, String> {
        match &config {
            Ok(None) => Ok(read_file(&node_tasks_path(project, &t.id))),
            Ok(Some(_)) if t.task_scope.is_empty() => Ok(Vec::new()),
            Ok(Some(config)) => {
                let scope = Scope {
                    node: &t.id,
                    scope: &t.task_scope,
                };
                match read {
                    Read::Run => tracker::tasks_for(ctx, project, config, scope, false),
                    Read::Cached => {
                        tracker::cached_for(project, config, scope).unwrap_or(Ok(Vec::new()))
                    }
                }
            }
            Err(error) => Err(tracker::config_error(error)),
        }
    };
    let mut owners: Vec<OwnerList> = nodes
        .iter()
        .map(|t| OwnerList {
            id: t.id.clone(),
            title: t.title.clone(),
            tasks: node_list(t),
        })
        .collect();
    // A tracker task a scoped coordinator lists is that coordinator's.
    let scoped: Vec<Task> = owners
        .iter()
        .flat_map(|o| o.tasks.as_deref().unwrap_or_default().to_vec())
        .collect();
    let root = match &project_tasks {
        Tasks::File(list) => Ok(list.clone()),
        Tasks::Tracker(list) => Ok(list
            .iter()
            .filter(|t| !scoped.iter().any(|s| same(s, t)))
            .cloned()
            .collect()),
        Tasks::Unavailable(reason) => Err(reason.clone()),
    };
    owners.insert(
        0,
        OwnerList {
            id: ROOT_ID.into(),
            title: String::new(),
            tasks: root,
        },
    );
    let unowned = match &config {
        Ok(None) => unowned_files(project, &owners),
        _ => Vec::new(),
    };
    let found = match &config {
        Ok(Some(_)) => tracker::cached_gets(project),
        _ => Vec::new(),
    };
    Lists {
        project: project_tasks,
        owners,
        unowned,
        found,
        assigned: thread::list(project).iter().any(|t| !t.tasks.is_empty()),
    }
}

fn read_file(path: &std::path::Path) -> Vec<Task> {
    tasks::parse(&std::fs::read_to_string(path).unwrap_or_default())
}

/// The `nodes/<id>/TASKS.md` of nodes that own no list.
fn unowned_files(project: &Project, owners: &[OwnerList]) -> Vec<(String, Task)> {
    let Ok(entries) = std::fs::read_dir(project.dir().join("nodes")) else {
        return Vec::new();
    };
    let mut ids: Vec<String> = entries
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|id| thread::validate_id(id).is_ok() && !owners.iter().any(|o| &o.id == id))
        .collect();
    ids.sort();
    ids.into_iter()
        .flat_map(|id| {
            read_file(&node_tasks_path(project, &id))
                .into_iter()
                .map(move |task| (id.clone(), task))
        })
        .collect()
}

/// Someone on a task, with the nodes under them on it too.
#[derive(Serialize, Clone)]
pub struct Assignee {
    pub id: String,
    pub title: String,
    pub role: &'static str,
    /// Its group token as the ticker last recorded it, or `failed`.
    pub group: &'static str,
    /// The `Status:` line of its report, `null` without one.
    pub verdict: Option<report::Verdict>,
    pub assignees: Vec<Assignee>,
}

/// Who works on `task`, nested by the tree: a node under another node on
/// the same task is listed under it. `viewer` is left out, its nodes then
/// standing first.
fn assignees(project: &Project, threads: &[Thread], task: &Task, viewer: &str) -> Vec<Assignee> {
    let on: Vec<&Thread> = threads
        .iter()
        .filter(|t| t.id != viewer)
        .filter(|t| {
            task.thread.as_deref() == Some(t.id.as_str())
                || t.assigned().iter().any(|key| names(task, key))
        })
        .collect();
    let ids: HashSet<&str> = on.iter().map(|t| t.id.as_str()).collect();
    // The nearest ancestor that is on the task too, `None` at the top.
    let above = |t: &Thread| -> Option<String> {
        let mut parent = crate::organizations::parent_id(t).to_string();
        let mut steps = 0;
        while parent != ROOT_ID && steps < threads.len() {
            if ids.contains(parent.as_str()) {
                return Some(parent);
            }
            parent = threads
                .iter()
                .find(|p| p.id == parent)
                .map_or(ROOT_ID.to_string(), |p| {
                    crate::organizations::parent_id(p).to_string()
                });
            steps += 1;
        }
        None
    };
    fn under(
        project: &Project,
        on: &[&Thread],
        above: &dyn Fn(&Thread) -> Option<String>,
        parent: Option<&str>,
    ) -> Vec<Assignee> {
        on.iter()
            .filter(|t| above(t).as_deref() == parent)
            .map(|t| Assignee {
                id: t.id.clone(),
                title: t.title.clone(),
                role: t.role.as_str(),
                group: group(t),
                verdict: report::verdict(
                    &std::fs::read_to_string(thread::home_report_path(project, &t.id))
                        .unwrap_or_default(),
                ),
                assignees: under(project, on, above, Some(&t.id)),
            })
            .collect()
    }
    under(project, &on, &above, None)
}

fn group(t: &Thread) -> &'static str {
    match t.status {
        Status::Failed => "failed",
        Status::Starting => thread::Group::Working.token(),
        _ => threads::recorded_group(t).token(),
    }
}

/// `t-0001 (t-0003, t-0004)`.
fn words(assignees: &[Assignee]) -> String {
    assignees
        .iter()
        .map(|a| {
            if a.assignees.is_empty() {
                a.id.clone()
            } else {
                format!("{} ({})", a.id, words(&a.assignees))
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn view<'a>(project: &Project, threads: &[Thread], task: &'a Task, viewer: &str) -> TaskView<'a> {
    TaskView {
        task,
        assignees: assignees(project, threads, task, viewer),
    }
}

/// A task with who works on it.
struct TaskView<'a> {
    task: &'a Task,
    assignees: Vec<Assignee>,
}

impl TaskView<'_> {
    /// A coordinator node is on it: the owner handed it to that node.
    fn delegated(&self) -> bool {
        self.assignees.iter().any(|a| a.role == "coordinator")
    }

    fn json(&self, is_profile: &dyn Fn(&str) -> bool) -> Value {
        let mut json = tasks::to_json(self.task, is_profile);
        json["assignees"] = serde_json::to_value(&self.assignees).unwrap_or_default();
        json["delegated"] = self.delegated().into();
        json
    }

    /// `task list`'s line: `list<TAB>title<TAB>owner<TAB>thread`, a
    /// tracker's `<TAB>id`, then who works on it.
    fn line(&self) -> String {
        let t = self.task;
        let id = if t.id.is_empty() {
            String::new()
        } else {
            format!("\t{}", t.id)
        };
        let who = match words(&self.assignees) {
            w if self.delegated() => format!("delegated: {w}"),
            w => w,
        };
        format!(
            "{}\t{}\t{}\t{}{id}\t{who}",
            t.list,
            t.title,
            t.owner,
            t.thread.as_deref().unwrap_or_default()
        )
    }
}

fn is_profile(ctx: &Ctx) -> Result<impl Fn(&str) -> bool> {
    let config = crate::profiles::load(&ctx.config_dir)?;
    Ok(move |p: &str| config.get(p).is_some())
}

/// The project view in the `--json` contract: `{owners, unowned}`, `None`
/// for a project whose only list is the root's.
pub fn project_json(ctx: &Ctx, project: &Project, read: Read) -> Result<Option<Value>> {
    let lists = lists(ctx, project, read);
    if !lists.grouped() {
        return Ok(None);
    }
    Ok(Some(grouped_json(ctx, project, &lists)?))
}

fn grouped_json(ctx: &Ctx, project: &Project, lists: &Lists) -> Result<Value> {
    let threads = thread::list(project);
    let is_profile = is_profile(ctx)?;
    let view = |task| view(project, &threads, task, "");
    let owners: Vec<Value> = lists
        .owners
        .iter()
        .map(|owner| {
            let mut json = serde_json::json!({
                "id": owner.id,
                "title": owner.title,
                "tasks": owner.tasks.as_deref().unwrap_or_default().iter()
                    .map(|t| view(t).json(&is_profile)).collect::<Vec<_>>(),
            });
            if let Err(reason) = &owner.tasks {
                json["unavailable"] = reason.clone().into();
            }
            json
        })
        .collect();
    let unowned: Vec<Value> = lists
        .unowned
        .iter()
        .map(|(node, task)| {
            let mut json = view(task).json(&is_profile);
            json["owner_node"] = node.clone().into();
            json
        })
        .collect();
    Ok(serde_json::json!({"owners": owners, "unowned": unowned}))
}

/// `task list <slug> [--json]`: the root's tasks as before, and with
/// coordinator nodes every owner's, with who works on each.
pub fn print_project(ctx: &Ctx, slug: &str, json: bool) -> Result<()> {
    let project = Project::load(&ctx.root, slug)?;
    let lists = lists(ctx, &project, Read::Run);
    if let Tasks::Unavailable(reason) = &lists.project {
        bail!("{reason}");
    }
    if !lists.grouped() {
        return tasks::print_tasks(ctx, &project, lists.project.list(), json);
    }
    if json {
        let is_profile = is_profile(ctx)?;
        let mut document = serde_json::json!({
            "schema_version": threads::JSON_SCHEMA_VERSION,
            "project": project.slug,
            "tasks": lists.project.list().iter().map(|t| tasks::to_json(t, &is_profile)).collect::<Vec<_>>(),
        });
        let grouped = grouped_json(ctx, &project, &lists)?;
        document["owners"] = grouped["owners"].clone();
        document["unowned"] = grouped["unowned"].clone();
        return threads::print_json(&document);
    }
    let threads = thread::list(&project);
    let view = |task| view(&project, &threads, task, "");
    for owner in &lists.owners {
        if owner.title.is_empty() {
            println!("## {}", owner.id);
        } else {
            println!("## {} {}", owner.id, owner.title);
        }
        match &owner.tasks {
            Ok(list) => list.iter().for_each(|t| println!("{}", view(t).line())),
            Err(reason) => println!("{reason}"),
        }
    }
    if !lists.unowned.is_empty() {
        println!("## unowned");
        for (node, task) in &lists.unowned {
            println!("{}\t{node}", view(task).line());
        }
    }
    Ok(())
}

/// `task list <slug> --node <id> [--json]`: a coordinator's own tasks, then
/// those it is assigned to elsewhere (delegated to it), each with who works
/// on it under the coordinator.
pub fn print_node(ctx: &Ctx, slug: &str, node: &str, json: bool) -> Result<()> {
    let project = Project::load(&ctx.root, slug)?;
    let lists = lists(ctx, &project, Read::Run);
    let Some(owner) = lists.owners.iter().find(|o| o.id == node) else {
        bail!(
            "`{node}` is no open coordinator of `{slug}`; the root and coordinator nodes own task lists"
        );
    };
    let own = match &owner.tasks {
        Ok(list) => list,
        Err(reason) => bail!("{reason}"),
    };
    let threads = thread::list(&project);
    let record = threads.iter().find(|t| t.id == node);
    let mine = |task: &Task| {
        record.is_some_and(|t| {
            task.thread.as_deref() == Some(t.id.as_str())
                || t.assigned().iter().any(|key| names(task, key))
        })
    };
    let mut rows: Vec<(&str, &str, TaskView)> = own
        .iter()
        .map(|task| {
            (
                "own",
                node,
                TaskView {
                    task,
                    assignees: assignees(&project, &threads, task, node),
                },
            )
        })
        .collect();
    let catalog = lists.catalog();
    for (owner, task) in &catalog {
        if *owner != node && mine(task) && !rows.iter().any(|(_, _, r)| same(r.task, task)) {
            rows.push((
                "assigned",
                owner,
                TaskView {
                    task,
                    assignees: assignees(&project, &threads, task, node),
                },
            ));
        }
    }
    // A tracker task this node is on that no list printed: by its id.
    let mut fetched = Vec::new();
    if let (Some(record), Ok(Some(config))) = (record, project.tracker(&ctx.config_dir)) {
        for key in record.assigned() {
            if catalog.iter().any(|(_, t)| names(t, key)) {
                continue;
            }
            if let Ok(Some(task)) = tracker::get(ctx, &project, &config, key) {
                fetched.push(task);
            }
        }
    }
    for task in &fetched {
        rows.push((
            "assigned",
            "",
            TaskView {
                task,
                assignees: assignees(&project, &threads, task, node),
            },
        ));
    }
    let title = record.map(|t| t.title.clone()).unwrap_or_default();
    if json {
        let is_profile = is_profile(ctx)?;
        return threads::print_json(&serde_json::json!({
            "schema_version": threads::JSON_SCHEMA_VERSION,
            "project": project.slug,
            "node": {"id": node, "title": title},
            "tasks": rows.iter().map(|(via, owner, row)| {
                let mut json = row.json(&is_profile);
                json["via"] = (*via).into();
                json["owner_node"] = (*owner).into();
                json
            }).collect::<Vec<_>>(),
        }));
    }
    for (via, _, row) in &rows {
        println!("{via}\t{}", row.line());
    }
    Ok(())
}

/// The task `key` names, found the way `--from-task` finds one, as a record
/// keeps it: a tracker task's id, a TASKS.md task's title as written in the
/// one list that has it. A tracker task no list prints is asked for by id.
pub fn find_key(ctx: &Ctx, project: &Project, key: &str) -> Result<String> {
    match tasks::load(ctx, project, true) {
        Tasks::Tracker(list) => match tasks::find_tracked(list, key, &project.slug) {
            Ok(task) => Ok(task.id),
            Err(error) => {
                let config = project.tracker(&ctx.config_dir)?;
                match config.map(|c| tracker::get(ctx, project, &c, key.trim())) {
                    Some(Ok(Some(task))) => Ok(task.id),
                    Some(Err(reason)) => bail!("{reason}"),
                    _ => Err(error),
                }
            }
        },
        Tasks::File(_) => {
            let lists = lists(ctx, project, Read::Cached);
            let found: Vec<(&str, &Task)> = lists
                .owners
                .iter()
                .flat_map(|o| {
                    o.tasks
                        .as_deref()
                        .unwrap_or_default()
                        .iter()
                        .map(|t| (o.id.as_str(), t))
                })
                .filter(|(_, t)| names(t, key))
                .collect();
            match found.as_slice() {
                [] => bail!(
                    "no task list of `{}` has a task titled \"{key}\"; `task list {}` shows them",
                    project.slug,
                    project.slug
                ),
                [(_, task)] => Ok(task.title.clone()),
                more => bail!(
                    "{} tasks are titled \"{key}\" ({}); tell them apart first",
                    more.len(),
                    more.iter()
                        .map(|(owner, t)| format!("{owner} line {}", t.line))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            }
        }
        Tasks::Unavailable(reason) => bail!("{reason}; nothing was changed"),
    }
}

/// `node assign` (`on`) and `node unassign`: the record's tasks only.
pub fn assign(ctx: &Ctx, slug: &str, id: &str, key: &str, on: bool) -> Result<()> {
    let project = Project::load(&ctx.root, slug)?;
    let record = thread::load(&project, id)?;
    if !on {
        let held = record
            .assigned()
            .iter()
            .any(|k| k.eq_ignore_ascii_case(key.trim()));
        if !held {
            bail!("{id} is not on task \"{key}\"");
        }
        thread::update(&project, id, |t| {
            t.tasks.retain(|k| !k.eq_ignore_ascii_case(key.trim()));
            if t.task_id.eq_ignore_ascii_case(key.trim()) {
                t.task_id.clear();
            }
        })?;
        println!("{id} is off task {}", key.trim());
        return Ok(());
    }
    if record.status == Status::Resolved {
        bail!("{id} is resolved; reopen it first");
    }
    let found = find_key(ctx, &project, key)?;
    thread::update(&project, id, |t| {
        if !t.assigned().iter().any(|k| k.eq_ignore_ascii_case(&found)) {
            t.tasks.push(found.clone());
        }
    })?;
    println!("{id} is on task {found}");
    Ok(())
}

/// `node set --task-scope`: what the tracker's `list` gets for this
/// coordinator; empty, it owns no tracker list.
pub fn set_scope(ctx: &Ctx, slug: &str, id: &str, scope: &str) -> Result<()> {
    let project = Project::load(&ctx.root, slug)?;
    let record = thread::load(&project, id)?;
    if record.role != NodeRole::Coordinator {
        bail!("{id} is a worker; only a coordinator owns a task list");
    }
    let scope = scope.trim();
    if scope.chars().any(char::is_control) {
        bail!("a task scope is one line of text");
    }
    thread::update(&project, id, |t| t.task_scope = scope.to_string())?;
    if scope.is_empty() {
        println!("{id} has no task scope; with a tracker it owns no list");
    } else {
        println!("{id}'s task scope is now `{scope}`");
    }
    Ok(())
}

/// Asks the tracker's `get` for the tasks a status file links or a node is
/// assigned to that no cached list has, so readers that run nothing (asks,
/// the popup, `overview`) know them. At most 16 per call; each answer is
/// cached for `cache_secs`. Nothing without a `get` command.
pub fn find_missing(ctx: &Ctx, project: &Project) {
    let Ok(Some(config)) = project.tracker(&ctx.config_dir) else {
        return;
    };
    if config.get.is_empty() {
        return;
    }
    let known = tasks::on_disk(ctx, project);
    let records = crate::asks::Records::load(ctx, project);
    let mut wanted: Vec<String> = crate::asks::sheets(project, &records)
        .into_iter()
        .flat_map(|sheet| sheet.status.asks)
        .flat_map(|ask| ask.links)
        .filter(|link| link.kind == crate::asks::LinkKind::Task && link.state == "missing")
        .filter_map(|link| link.text.strip_prefix("task:").map(str::to_string))
        .collect();
    for t in thread::list(project) {
        wanted.extend(t.assigned().into_iter().map(str::to_string));
    }
    let mut seen = HashSet::new();
    for id in wanted
        .into_iter()
        .filter(|id| !known.iter().any(|t| t.id == *id))
        .filter(|id| seen.insert(id.clone()))
        .take(16)
    {
        let _ = tracker::get(ctx, project, &config, &id);
    }
}

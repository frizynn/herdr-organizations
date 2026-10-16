//! TASKS.md: `## List` headings, one `- [ ] title (owner) · t-0007` line per
//! task, and an optional description indented under its task line. The
//! coordinator writes it; the binary only appends ` · <id>` to a task line
//! when a thread is delegated from it (`link`). A project whose config.toml
//! names a tracker takes the same tasks from it instead (`tracker`), and
//! TASKS.md is ignored there.

use anyhow::{Result, bail};
use std::fmt;
use std::path::Path;

/// Who a task is assigned to: the text in its `(...)`. The thread running it
/// is status, kept apart in `Task::thread`.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum Owner {
    /// No `(...)`, or an old `(agent)`.
    #[default]
    Unassigned,
    /// `(me)`: the user.
    Me,
    /// `(codex-fast)` on this machine, `(@m1)` for whatever profile m1's
    /// coordinator picks, or `(codex-fast@m1)`. Neither set is an old
    /// `(agent → t-0007)`: this machine's default thread profile.
    Agent {
        profile: Option<String>,
        machine: Option<String>,
    },
    /// Free text that is not name-shaped, such as `Priya Rao`: a person the
    /// user named. A name-shaped owner that is no profile here (`Priya`)
    /// parses as `Agent` and is told apart by `is_person`.
    Person(String),
}

impl Owner {
    pub fn parse(text: &str) -> Owner {
        let text = text.trim();
        match text {
            "" | "agent" => return Owner::Unassigned,
            "me" => return Owner::Me,
            _ => {}
        }
        let (profile, machine) = match text.split_once('@') {
            Some((profile, machine)) => (profile, Some(machine)),
            None => (text, None),
        };
        let profile_ok = profile.is_empty() && machine.is_some()
            || crate::profiles::validate_name(profile).is_ok();
        if !profile_ok || machine.is_some_and(|m| !is_machine_name(m)) {
            return Owner::Person(text.to_string());
        }
        Owner::Agent {
            profile: (!profile.is_empty()).then(|| profile.to_string()),
            machine: machine.map(str::to_string),
        }
    }

    /// Whether this owner is a person: free text, or a bare name that
    /// `is_profile` does not know. `profile@machine` and `@machine` are
    /// always agents.
    pub fn is_person(&self, is_profile: impl Fn(&str) -> bool) -> bool {
        match self {
            Owner::Person(_) => true,
            Owner::Agent {
                profile: Some(profile),
                machine: None,
            } => !is_profile(profile),
            _ => false,
        }
    }
}

/// A machine label as Herdr and config.toml name one: letters, digits, `.`,
/// `_` and `-`.
pub fn is_machine_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('-')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
}

impl fmt::Display for Owner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Owner::Unassigned => Ok(()),
            Owner::Me => f.write_str("me"),
            Owner::Agent {
                profile: None,
                machine: None,
            } => f.write_str("default profile"),
            Owner::Agent { profile, machine } => {
                f.write_str(profile.as_deref().unwrap_or(""))?;
                match machine {
                    Some(m) => write!(f, "@{m}"),
                    None => Ok(()),
                }
            }
            Owner::Person(text) => f.write_str(text),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Task {
    /// Its line in TASKS.md, from 1.
    pub line: usize,
    /// `- [x]`: checked done, which the skill says to delete instead.
    pub done: bool,
    pub list: String,
    pub title: String,
    pub owner: Owner,
    /// The thread running the task: ` · t-0007` after the owner.
    pub thread: Option<String>,
    /// The indented lines under the task line, with the indent removed.
    pub description: String,
    /// The tracker's id; empty for a TASKS.md task, which `line` names.
    pub id: String,
    /// The task's page in the tracker, empty when none.
    pub url: String,
}

impl Task {
    /// Where the task is kept, for messages: TASKS.md or the tracker.
    pub fn kept_in(&self) -> &'static str {
        if self.id.is_empty() {
            "TASKS.md"
        } else {
            "the tracker"
        }
    }
}

/// A project's tasks, and where they came from.
#[derive(Debug, Clone, PartialEq)]
pub enum Tasks {
    File(Vec<Task>),
    Tracker(Vec<Task>),
    /// A tracker is set but gave no list: why, in this binary's words.
    Unavailable(String),
}

impl Tasks {
    /// The tasks known now; none while the tracker is unavailable.
    pub fn list(&self) -> &[Task] {
        match self {
            Tasks::File(list) | Tasks::Tracker(list) => list,
            Tasks::Unavailable(_) => &[],
        }
    }

    /// `file`, `tracker` or `unavailable`, as `project status --json` names it.
    pub fn source(&self) -> &'static str {
        match self {
            Tasks::File(_) => "file",
            Tasks::Tracker(_) => "tracker",
            Tasks::Unavailable(_) => "unavailable",
        }
    }
}

/// The project's tasks: its tracker's when config.toml names one (cached for
/// `cache_secs` unless `fresh`), else TASKS.md's.
pub fn load(ctx: &crate::paths::Ctx, project: &crate::project::Project, fresh: bool) -> Tasks {
    match project.tracker(&ctx.config_dir) {
        Ok(None) => Tasks::File(parse(&read(&project.dir()))),
        Ok(Some(config)) => match crate::tracker::tasks(ctx, project, &config, fresh) {
            Ok(list) => Tasks::Tracker(list),
            Err(reason) => Tasks::Unavailable(reason),
        },
        Err(error) => Tasks::Unavailable(format!("tracker unavailable: {error:#}")),
    }
}

fn is_task_line(line: &str) -> bool {
    line.trim_start().starts_with("- [")
        && line
            .trim_start()
            .get(3..5)
            .is_some_and(|s| s.ends_with(']'))
}

fn indent(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

/// The description lines that follow the task line at `index`: indented,
/// not themselves task lines; blank lines count only between such lines.
fn body(lines: &[&str], index: usize) -> (Vec<String>, usize) {
    let base = indent(lines[index]);
    let mut end = index + 1;
    let mut last = index;
    while end < lines.len() {
        let line = lines[end];
        if line.trim().is_empty() {
            end += 1;
            continue;
        }
        if indent(line) <= base || is_task_line(line) || line.starts_with('#') {
            break;
        }
        last = end;
        end += 1;
    }
    let taken: Vec<&str> = lines[index + 1..=last].to_vec();
    let cut = taken
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| indent(l))
        .min()
        .unwrap_or(0);
    let text = taken
        .iter()
        .map(|l| {
            if l.trim().is_empty() {
                String::new()
            } else {
                l[cut..].trim_end().to_string()
            }
        })
        .collect();
    (text, last + 1)
}

pub fn parse(text: &str) -> Vec<Task> {
    let lines: Vec<&str> = text.lines().collect();
    let mut list = String::new();
    let mut tasks = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if let Some(heading) = line.strip_prefix("## ") {
            list = heading.trim().to_string();
            i += 1;
            continue;
        }
        if !is_task_line(line) {
            i += 1;
            continue;
        }
        let (title, owner, thread) = split_line(line.trim_start()[5..].trim());
        let (description, next) = body(&lines, i);
        tasks.push(Task {
            line: i + 1,
            done: matches!(line.trim_start().as_bytes()[3], b'x' | b'X'),
            list: list.clone(),
            title,
            owner,
            thread,
            description: description.join("\n"),
            ..Task::default()
        });
        i = next;
    }
    tasks
}

fn is_thread_id(text: &str) -> bool {
    text.strip_prefix("t-")
        .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
}

/// `Title (owner) · t-0007` into its parts. An old `(agent → t-0007)` is
/// this machine's default profile with thread t-0007.
fn split_line(rest: &str) -> (String, Owner, Option<String>) {
    let (rest, mut thread) = match rest.rsplit_once('·') {
        Some((head, id)) if is_thread_id(id.trim()) => {
            (head.trim_end(), Some(id.trim().to_string()))
        }
        _ => (rest, None),
    };
    let Some(open) = rest.rfind('(').filter(|_| rest.ends_with(')')) else {
        return (rest.to_string(), Owner::Unassigned, thread);
    };
    let title = rest[..open].trim().to_string();
    let inner = rest[open + 1..rest.len() - 1].trim();
    let owner = match inner.split_once('→') {
        Some((who, id)) if is_thread_id(id.trim()) => {
            thread = thread.or_else(|| Some(id.trim().to_string()));
            match who.trim() {
                "agent" => Owner::Agent {
                    profile: None,
                    machine: None,
                },
                who => Owner::parse(who),
            }
        }
        _ => Owner::parse(inner),
    };
    (title, owner, thread)
}

pub fn read(project_dir: &Path) -> String {
    std::fs::read_to_string(project_dir.join("TASKS.md")).unwrap_or_default()
}

/// `task list [--json]`: the JSON document, or one
/// `list<TAB>title<TAB>owner<TAB>thread` line per task, a tracker's with
/// `<TAB>id` after them.
pub fn print_list(ctx: &crate::paths::Ctx, slug: &str, json: bool) -> Result<()> {
    let project = crate::project::Project::load(&ctx.root, slug)?;
    let tasks = match load(ctx, &project, false) {
        Tasks::Unavailable(reason) => bail!("{reason}"),
        tasks => tasks.list().to_vec(),
    };
    if json {
        let config = crate::profiles::load(&ctx.config_dir)?;
        let is_profile = |p: &str| config.get(p).is_some();
        return crate::threads::print_json(&serde_json::json!({
            "schema_version": crate::threads::JSON_SCHEMA_VERSION,
            "project": project.slug,
            "tasks": tasks.iter().map(|t| to_json(t, is_profile)).collect::<Vec<_>>(),
        }));
    }
    for t in &tasks {
        let id = if t.id.is_empty() {
            String::new()
        } else {
            format!("\t{}", t.id)
        };
        println!(
            "{}\t{}\t{}\t{}{id}",
            t.list,
            t.title,
            t.owner,
            t.thread.as_deref().unwrap_or_default()
        );
    }
    Ok(())
}

/// A task in the `task list --json` contract (docs/json.md). `is_profile`
/// tells a bare name that is a profile from a person's. `id` and `url` are
/// only a tracker task's, so TASKS.md's document is what it was before.
pub fn to_json(task: &Task, is_profile: impl Fn(&str) -> bool) -> serde_json::Value {
    let person = task.owner.is_person(is_profile);
    let (kind, profile, machine) = match &task.owner {
        Owner::Unassigned => ("unassigned", None, None),
        Owner::Me => ("me", None, None),
        Owner::Agent { profile, machine } if !person => {
            ("agent", profile.as_deref(), machine.as_deref())
        }
        Owner::Agent { .. } | Owner::Person(_) => ("person", None, None),
    };
    let mut json = serde_json::json!({
        "list": task.list,
        "title": task.title,
        "owner": {
            "kind": kind,
            "profile": profile.unwrap_or_default(),
            "machine": machine.unwrap_or_default(),
            "name": if person { task.owner.to_string() } else { String::new() },
        },
        "thread": task.thread.as_deref().unwrap_or_default(),
        "description": task.description,
    });
    if !task.id.is_empty() {
        json["id"] = task.id.clone().into();
        json["url"] = task.url.clone().into();
    }
    json
}

/// TASKS.md for `hp context`: every description folded into one line, so a
/// long body costs a few tokens; the coordinator reads the file for the rest.
pub fn compact(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        out.push(lines[i].to_string());
        if !is_task_line(lines[i]) {
            i += 1;
            continue;
        }
        let (description, next) = body(&lines, i);
        if let Some(notes) = notes_line(description.iter().map(String::as_str), "TASKS.md") {
            out.push(format!("{}  {notes}", " ".repeat(indent(lines[i]))));
        }
        i = next;
    }
    out.join("\n")
}

/// `notes: <first line> (+N more lines in <kept_in>)`, `None` without notes.
fn notes_line<'a>(description: impl Iterator<Item = &'a str>, kept_in: &str) -> Option<String> {
    let filled: Vec<&str> = description.filter(|l| !l.trim().is_empty()).collect();
    let first = filled.first()?.trim();
    let mut preview: String = first.chars().take(60).collect();
    if first.chars().count() > 60 {
        preview.push('…');
    }
    let more = if filled.len() > 1 {
        format!(" (+{} more lines in {kept_in})", filled.len() - 1)
    } else {
        String::new()
    };
    Some(format!("notes: {preview}{more}"))
}

/// A tracker's tasks for `hp context`, shaped like TASKS.md: each list under
/// a `###` heading (below the digest's own `##` sections), each task on one
/// line with its id, each description folded into one `notes:` line.
pub fn compact_tracked(tasks: &[Task]) -> String {
    let mut out = Vec::new();
    let mut list = None;
    for task in tasks {
        if list != Some(&task.list) {
            list = Some(&task.list);
            if !task.list.is_empty() {
                out.push(format!("### {}", task.list));
            }
        }
        let owner = match task.owner.to_string() {
            owner if owner.is_empty() => owner,
            owner => format!(" ({owner})"),
        };
        let thread = task
            .thread
            .as_deref()
            .map_or(String::new(), |id| format!(" · {id}"));
        out.push(format!(
            "- [{}] [{}] {}{owner}{thread}",
            if task.done { "x" } else { " " },
            task.id,
            task.title
        ));
        if let Some(notes) = notes_line(task.description.lines(), "the tracker") {
            out.push(format!("  {notes}"));
        }
    }
    out.join("\n")
}

/// What `--from-task` starts a thread with: the task as written, its owner's
/// profile and machine (`launch_for`), and the coordinator's text followed by
/// the task's notes.
pub struct Delegation {
    pub task: Task,
    pub profile: Option<String>,
    pub machine: Option<String>,
    pub text: String,
}

/// `key` is a TASKS.md title, or a tracker task's id or title; a tracker is
/// asked again rather than trusting its cache, which may predate the task.
pub fn delegation(
    ctx: &crate::paths::Ctx,
    project: &crate::project::Project,
    key: &str,
    profile: Option<String>,
    machine: Option<String>,
    text: &str,
) -> Result<Delegation> {
    let task = match load(ctx, project, true) {
        Tasks::File(list) => find_titled(list, key)?,
        Tasks::Tracker(list) => find_tracked(list, key, &project.slug)?,
        Tasks::Unavailable(reason) => {
            bail!("{reason}; --from-task needs it to find the task, so nothing was started")
        }
    };
    let config = crate::profiles::load(&ctx.config_dir)?;
    let (profile, machine) = launch_for(&task, |p| config.get(p).is_some(), profile, machine)?;
    let text = with_notes(&task, text);
    Ok(Delegation {
        task,
        profile,
        machine,
        text,
    })
}

/// The coordinator's text, then the task's description when it has one.
fn with_notes(task: &Task, text: &str) -> String {
    if task.description.trim().is_empty() {
        return text.to_string();
    }
    format!(
        "{}\n\n## Notes from the task list\n\n{}\n",
        text.trim_end(),
        task.description.trim()
    )
}

/// The one TASKS.md task titled `title`, ignoring case. Two tasks of that
/// title are refused: a link to the first could be the wrong one.
pub fn find(tasks_md: &str, title: &str) -> Result<Task> {
    find_titled(parse(tasks_md), title)
}

fn find_titled(tasks: Vec<Task>, title: &str) -> Result<Task> {
    let wanted = title.trim().to_lowercase();
    let mut found: Vec<Task> = tasks
        .into_iter()
        .filter(|t| t.title.to_lowercase() == wanted)
        .collect();
    match found.len() {
        0 => bail!(
            "TASKS.md has no task titled \"{title}\"; --from-task takes the title exactly as it is written there"
        ),
        1 => Ok(found.remove(0)),
        n => bail!(
            "TASKS.md has {n} tasks titled \"{title}\" (lines {}); ask the coordinator to tell them apart first",
            found
                .iter()
                .map(|t| t.line.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// The one tracker task whose id is `key`, else whose title is, ignoring
/// case; a title two tasks share is refused, as in TASKS.md.
fn find_tracked(tasks: Vec<Task>, key: &str, slug: &str) -> Result<Task> {
    let wanted = key.trim().to_lowercase();
    let matching = |field: fn(&Task) -> &str| -> Vec<Task> {
        tasks
            .iter()
            .filter(|t| field(t).to_lowercase() == wanted)
            .cloned()
            .collect()
    };
    let mut found = matching(|t| &t.id);
    if found.is_empty() {
        found = matching(|t| &t.title);
    }
    match found.len() {
        0 => bail!(
            "the tracker lists no task with the id or title \"{key}\"; `task list {slug}` shows them"
        ),
        1 => Ok(found.remove(0)),
        n => bail!(
            "the tracker lists {n} tasks for \"{key}\" ({}); --from-task takes one of these ids",
            found
                .iter()
                .map(|t| t.id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// TASKS.md with ` · <id>` after the task titled `title`, in place of the
/// thread it named before. Every other byte stays as it was.
pub fn link(tasks_md: &str, title: &str, id: &str) -> Result<String> {
    let task = find(tasks_md, title)?;
    let mut out = String::with_capacity(tasks_md.len() + id.len() + 4);
    for (index, line) in tasks_md.split_inclusive('\n').enumerate() {
        if index + 1 != task.line {
            out.push_str(line);
            continue;
        }
        let text = line.trim_end_matches(['\n', '\r']);
        let head = match text.rsplit_once('·') {
            Some((head, old)) if is_thread_id(old.trim()) => head.trim_end(),
            _ => text.trim_end(),
        };
        out.push_str(&format!("{head} · {id}"));
        out.push_str(&line[text.len()..]);
    }
    Ok(out)
}

/// The open TASKS.md tasks that name thread `id` as the one running them.
#[cfg(test)]
pub fn open_on(tasks_md: &str, id: &str) -> Vec<Task> {
    open_in(&parse(tasks_md), id, "")
}

/// The open tasks of `tasks` that name thread `id`, or whose tracker id is
/// `task_id`, the one the thread was started from.
pub fn open_in(tasks: &[Task], id: &str, task_id: &str) -> Vec<Task> {
    tasks
        .iter()
        .filter(|t| {
            !t.done && (t.thread.as_deref() == Some(id) || !task_id.is_empty() && t.id == task_id)
        })
        .cloned()
        .collect()
}

/// The `--profile` and `--machine` a delegated task starts with: its owner's,
/// unless the command line gives its own. A flag that contradicts the owner is
/// refused, so the task line and the thread never disagree, and so is a
/// person's task (`is_profile` tells a bare name that is no profile).
pub fn launch_for(
    task: &Task,
    is_profile: impl Fn(&str) -> bool,
    profile: Option<String>,
    machine: Option<String>,
) -> Result<(Option<String>, Option<String>)> {
    let title = &task.title;
    let person = || {
        anyhow::anyhow!(
            "\"{title}\" belongs to {}, a person; people's tasks are never delegated. Change its owner in {} first if the user asks",
            task.owner,
            task.kept_in()
        )
    };
    match &task.owner {
        Owner::Unassigned => Ok((profile, machine)),
        Owner::Me => bail!(
            "\"{title}\" is the user's own task (me); change its owner in {} before delegating it",
            task.kept_in()
        ),
        Owner::Person(_) => Err(person()),
        Owner::Agent { .. } if task.owner.is_person(is_profile) => Err(person()),
        Owner::Agent {
            profile: owned_profile,
            machine: owned_machine,
        } => {
            if let (Some(flag), Some(owned)) = (&profile, owned_profile)
                && flag != owned
            {
                bail!(
                    "\"{title}\" is assigned to profile `{owned}`, not `{flag}`; change its owner in {} first, or leave out --profile",
                    task.kept_in()
                );
            }
            if machine != *owned_machine && machine.is_some() {
                let owned = owned_machine
                    .as_deref()
                    .map_or("this machine".to_string(), |m| format!("machine `{m}`"));
                bail!(
                    "\"{title}\" is assigned to {owned}, not `{}`; change its owner in {} first, or leave out --machine",
                    machine.unwrap_or_default(),
                    task.kept_in()
                );
            }
            Ok((
                profile.or_else(|| owned_profile.clone()),
                owned_machine.clone(),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT: &str = "# Tasks\n\n## Backlog\n- [ ] Write the docs (me)\n- [ ] Fix login (agent → t-0007)\n  Users on Safari get logged out.\n\n  Bugs:\n    - cookie is dropped\n  See https://example.com/issue/4\n- [ ] Plain line\n\n## Later\n- [x] Old (agent)\n  - [ ] A subtask stays a task\n";

    fn agent(profile: Option<&str>, machine: Option<&str>) -> Owner {
        Owner::Agent {
            profile: profile.map(str::to_string),
            machine: machine.map(str::to_string),
        }
    }

    #[test]
    fn one_line_tasks_keep_parsing_as_before() {
        let tasks = parse("# Tasks\n\n## Backlog\n- [ ] Write the docs (me)\n- [ ] Plain line\n");
        assert_eq!(tasks.len(), 2);
        assert_eq!(
            (
                tasks[0].title.as_str(),
                &tasks[0].owner,
                tasks[0].description.as_str()
            ),
            ("Write the docs", &Owner::Me, "")
        );
        assert_eq!(tasks[1].owner, Owner::Unassigned);
    }

    #[test]
    fn owners_are_a_profile_a_machine_or_both_and_the_thread_stands_apart() {
        let tasks = parse(
            "## A\n- [ ] One (codex-fast)\n- [ ] Two (@m1)\n- [ ] Three (codex-fast@m1) · t-0007\n- [ ] Four (me)\n- [ ] Five\n- [ ] Six (Priya Rao)\n- [ ] Fix (the) login (claude)\n- [ ] Unassigned but running · t-0009\n",
        );
        let owners: Vec<&Owner> = tasks.iter().map(|t| &t.owner).collect();
        assert_eq!(
            owners,
            [
                &agent(Some("codex-fast"), None),
                &agent(None, Some("m1")),
                &agent(Some("codex-fast"), Some("m1")),
                &Owner::Me,
                &Owner::Unassigned,
                &Owner::Person("Priya Rao".into()),
                &agent(Some("claude"), None),
                &Owner::Unassigned
            ]
        );
        assert_eq!(tasks[2].thread.as_deref(), Some("t-0007"));
        assert_eq!(tasks[2].title, "Three");
        assert_eq!(tasks[6].title, "Fix (the) login");
        assert_eq!(
            (tasks[7].title.as_str(), tasks[7].thread.as_deref()),
            ("Unassigned but running", Some("t-0009"))
        );
        let shown: Vec<String> = tasks.iter().map(|t| t.owner.to_string()).collect();
        assert_eq!(
            shown,
            [
                "codex-fast",
                "@m1",
                "codex-fast@m1",
                "me",
                "",
                "Priya Rao",
                "claude",
                ""
            ]
        );
        assert!(matches!(Owner::parse("a@"), Owner::Person(_)));
        assert!(matches!(Owner::parse("@"), Owner::Person(_)));
        assert!(matches!(Owner::parse("a@b@c"), Owner::Person(_)));
    }

    #[test]
    fn old_agent_owners_still_read() {
        let tasks = parse("## A\n- [ ] Old (agent)\n- [ ] Running (agent → t-0007)\n");
        assert_eq!(tasks[0].owner, Owner::Unassigned);
        assert_eq!(
            (&tasks[1].owner, tasks[1].thread.as_deref()),
            (&agent(None, None), Some("t-0007"))
        );
        assert_eq!(tasks[1].owner.to_string(), "default profile");
    }

    #[test]
    fn delegation_takes_profile_and_machine_from_the_owner() {
        let tasks = parse(
            "## A\n- [ ] P (fast)\n- [ ] M (@m1)\n- [ ] PM (fast@m1)\n- [ ] U\n- [ ] Me (me)\n- [ ] X (Bob Smith)\n",
        );
        let s = |v: &str| Some(v.to_string());
        let known = |p: &str| ["fast", "deep"].contains(&p);
        assert_eq!(
            launch_for(&tasks[0], known, None, None).unwrap(),
            (s("fast"), None)
        );
        assert_eq!(
            launch_for(&tasks[1], known, s("deep"), None).unwrap(),
            (s("deep"), s("m1"))
        );
        assert_eq!(
            launch_for(&tasks[2], known, None, s("m1")).unwrap(),
            (s("fast"), s("m1"))
        );
        assert_eq!(
            launch_for(&tasks[3], known, s("deep"), s("m2")).unwrap(),
            (s("deep"), s("m2"))
        );
        assert!(
            launch_for(&tasks[0], known, s("deep"), None)
                .unwrap_err()
                .to_string()
                .contains("assigned to profile `fast`")
        );
        assert!(
            launch_for(&tasks[0], known, None, s("m1"))
                .unwrap_err()
                .to_string()
                .contains("assigned to this machine")
        );
        assert!(launch_for(&tasks[2], known, None, s("m2")).is_err());
        assert!(launch_for(&tasks[4], known, None, None).is_err());
        assert!(
            launch_for(&tasks[5], known, None, None)
                .unwrap_err()
                .to_string()
                .contains("a person")
        );
        // A bare name that is no profile here is a person too.
        let priya = &parse("- [ ] Call the bank (Priya)\n")[0];
        assert!(
            launch_for(priya, known, None, None)
                .unwrap_err()
                .to_string()
                .contains("belongs to Priya, a person")
        );
    }

    #[test]
    fn a_bare_name_that_is_no_profile_is_a_person() {
        let known = |p: &str| p == "claude";
        assert!(Owner::parse("Priya").is_person(known));
        assert!(Owner::parse("Priya Rao").is_person(known));
        assert!(!Owner::parse("claude").is_person(known));
        assert!(!Owner::parse("Priya@m1").is_person(known));
        assert!(!Owner::parse("@m1").is_person(known));
        assert!(!Owner::parse("me").is_person(known));
        assert!(!Owner::parse("").is_person(known));
        let tasks = parse("## A\n- [ ] Call the bank (Elias)\n");
        assert_eq!(tasks[0].owner.to_string(), "Elias");
    }

    #[test]
    fn indented_lines_under_a_task_are_its_description() {
        let tasks = parse(TEXT);
        assert_eq!(tasks.len(), 5);
        assert_eq!(tasks[1].thread.as_deref(), Some("t-0007"));
        assert_eq!(
            tasks[1].description,
            "Users on Safari get logged out.\n\nBugs:\n  - cookie is dropped\nSee https://example.com/issue/4"
        );
        assert_eq!(tasks[2].description, "");
        assert_eq!(tasks[3].list, "Later");
        assert_eq!(tasks[4].title, "A subtask stays a task");
        assert_eq!(tasks[3].description, "");
    }

    #[test]
    fn a_blank_line_ends_the_description_when_nothing_indented_follows() {
        let tasks = parse("## A\n- [ ] One\n  note\n\nloose text\n- [ ] Two\n");
        assert_eq!(tasks[0].description, "note");
        assert_eq!(tasks[1].description, "");
    }

    #[test]
    fn compact_folds_each_description_into_one_line() {
        let text = compact(TEXT);
        assert!(text.contains("- [ ] Fix login (agent → t-0007)\n  notes: Users on Safari get logged out. (+3 more lines in TASKS.md)\n- [ ] Plain line"), "{text}");
        assert!(!text.contains("cookie"));
        assert!(text.contains("- [ ] Write the docs (me)\n- [ ] Fix login"));
        assert_eq!(compact("## B\n- [ ] One\n"), "## B\n- [ ] One");
        let long = compact(&format!("- [ ] T\n  {}\n", "x".repeat(80)));
        assert!(
            long.ends_with(&format!("notes: {}…", "x".repeat(60))),
            "{long}"
        );
    }

    #[test]
    fn delegation_appends_the_description_to_the_task() {
        let task = with_notes(
            &find(TEXT, "fix login").unwrap(),
            "Fix the Safari logout.\n",
        );
        assert_eq!(
            task,
            "Fix the Safari logout.\n\n## Notes from the task list\n\nUsers on Safari get logged out.\n\nBugs:\n  - cookie is dropped\nSee https://example.com/issue/4\n"
        );
        assert_eq!(
            with_notes(&find(TEXT, "Plain line").unwrap(), "Do it."),
            "Do it."
        );
        assert!(find(TEXT, "Nope").is_err());
    }

    #[test]
    fn each_task_knows_its_line_and_whether_it_is_checked() {
        let tasks = parse(TEXT);
        let seen: Vec<(usize, bool)> = tasks.iter().map(|t| (t.line, t.done)).collect();
        assert_eq!(
            seen,
            [(4, false), (5, false), (11, false), (14, true), (15, false)]
        );
        assert!(parse("- [X] Upper\n")[0].done);
    }

    #[test]
    fn link_sets_the_thread_after_the_owner_and_keeps_every_other_byte() {
        let text = "# Tasks\r\n\n## A\n- [ ] Fix login (claude)\n  notes stay\n- [ ] Ship · t-0002\n- [ ] Old (agent → t-0007)\n- [ ] Last";
        let linked = link(text, "fix login", "t-0009").unwrap();
        assert_eq!(
            linked,
            text.replace(
                "- [ ] Fix login (claude)\n",
                "- [ ] Fix login (claude) · t-0009\n"
            )
        );
        // A new thread replaces the one the line named.
        assert!(
            link(text, "Ship", "t-0010")
                .unwrap()
                .contains("- [ ] Ship · t-0010\n")
        );
        // An old arrow owner keeps its text; the suffix wins when parsed.
        let old = link(text, "Old", "t-0011").unwrap();
        assert!(old.contains("- [ ] Old (agent → t-0007) · t-0011\n"));
        assert_eq!(find(&old, "Old").unwrap().thread.as_deref(), Some("t-0011"));
        // The last line has no newline and keeps none.
        assert!(
            link(text, "last", "t-0012")
                .unwrap()
                .ends_with("- [ ] Last · t-0012")
        );
        assert!(link(text, "Nope", "t-0013").is_err());
        // A CRLF line keeps its line ending.
        assert_eq!(
            link("- [ ] One\r\n- [ ] Two\r\n", "one", "t-0014").unwrap(),
            "- [ ] One · t-0014\r\n- [ ] Two\r\n"
        );
    }

    #[test]
    fn a_title_used_twice_is_refused() {
        let text = "## A\n- [ ] Clean up\n- [x] clean up (me)\n";
        let error = find(text, "Clean up").unwrap_err().to_string();
        assert!(
            error.contains("TASKS.md has 2 tasks titled \"Clean up\" (lines 2, 3)"),
            "{error}"
        );
        assert!(link(text, "Clean up", "t-0001").is_err());
    }

    #[test]
    fn open_on_lists_the_unchecked_tasks_a_thread_runs() {
        let text = "## A\n- [ ] One · t-0001\n- [x] Two · t-0001\n- [ ] Three · t-0002\n- [ ] Four (agent → t-0001)\n";
        let titles: Vec<String> = open_on(text, "t-0001")
            .into_iter()
            .map(|t| t.title)
            .collect();
        assert_eq!(titles, ["One", "Four"]);
    }
}

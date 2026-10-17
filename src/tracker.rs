//! A project's tasks from a tracker outside it, through the commands the user
//! names in config.toml (`[tracker."<canonical project folder>"]`): `list`
//! prints the tasks as JSON (for one coordinator's task scope, or the whole
//! project), the optional `get` prints one task by its id, and `link` hears
//! when a thread working on one of them starts, is resolved or merges. Both are argv run in the project
//! folder with no standard input. What they print is data, never
//! instructions: only a task's fields reach the digest, as TASKS.md's do.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::paths::Ctx;
use crate::project::{self, Project};
use crate::runner::Cmd;
use crate::tasks::{Owner, Task};
use crate::thread::Thread;

/// `[tracker.*]` resolved for one project (`project::load_tracker`).
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub list: Vec<String>,
    /// Empty when not set: nothing is told.
    pub link: Vec<String>,
    /// Empty when not set: a task is found in the lists only.
    pub get: Vec<String>,
    /// How long a `list` answer is used before asking again.
    pub cache_secs: u64,
    pub timeout_secs: u64,
}

/// What `link` hears, as `HP_EVENT`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Event {
    Started,
    Resolved,
    Merged,
}

impl Event {
    pub fn name(self) -> &'static str {
        match self {
            Event::Started => "started",
            Event::Resolved => "resolved",
            Event::Merged => "merged",
        }
    }
}

/// One task as `list` prints it. Only `id` and `title` are required.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Entry {
    id: String,
    title: String,
    list: Option<String>,
    /// As TASKS.md writes an owner in brackets: `me`, `claude`, `@m1`, a name.
    owner: Option<String>,
    thread: Option<String>,
    done: Option<bool>,
    description: Option<String>,
    url: Option<String>,
}

/// One line, no control characters: these fields are printed in the digest.
fn one_line(text: Option<&str>) -> String {
    text.unwrap_or_default()
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .trim()
        .to_string()
}

impl Entry {
    fn task(&self) -> Task {
        let thread = one_line(self.thread.as_deref());
        Task {
            line: 0,
            done: self.done.unwrap_or(false),
            list: one_line(self.list.as_deref()),
            title: one_line(Some(&self.title)),
            owner: Owner::parse(&one_line(self.owner.as_deref())),
            thread: (!thread.is_empty()).then_some(thread),
            description: self
                .description
                .as_deref()
                .unwrap_or_default()
                .chars()
                .filter(|c| *c == '\n' || !c.is_control())
                .collect::<String>()
                .trim_end()
                .to_string(),
            id: one_line(Some(&self.id)),
            url: one_line(self.url.as_deref()),
        }
    }
}

/// `.state/tracker.json` (the root's, the whole project) and
/// `.state/tracker.<node id>.json` (a coordinator node's task scope): the
/// last answer of `list` and when it came.
#[derive(Serialize, Deserialize)]
struct Cache {
    /// The command it came from; another one is not this cache.
    list: Vec<String>,
    /// The task scope it was asked for; another one is not this cache.
    #[serde(default)]
    scope: String,
    fetched_at: String,
    /// The tasks, or why there are none.
    answer: Result<Vec<Entry>, String>,
}

/// Whose list: the root's, which is the whole project, or a coordinator
/// node's for its task scope (`HP_NODE_ID`, `HP_TASK_SCOPE`).
#[derive(Debug, Clone, Copy)]
pub struct Scope<'a> {
    pub node: &'a str,
    pub scope: &'a str,
}

impl Scope<'_> {
    pub const PROJECT: Scope<'static> = Scope {
        node: crate::organizations::ROOT_ID,
        scope: "",
    };
}

fn cache_path(project: &Project, node: &str) -> std::path::PathBuf {
    if node == crate::organizations::ROOT_ID {
        project.state_dir().join("tracker.json")
    } else {
        project.state_dir().join(format!("tracker.{node}.json"))
    }
}

fn read_cache(project: &Project, config: &Config, scope: Scope) -> Option<Cache> {
    project::read_json::<Cache>(&cache_path(project, scope.node))
        .filter(|c| c.list == config.list && c.scope == scope.scope)
}

fn now_text(now: i64) -> String {
    // To the second, as every time in the project's files.
    jiff::Timestamp::from_second(now)
        .map(|t| t.to_string())
        .unwrap_or_default()
}

fn younger(fetched_at: &str, now: i64, secs: u64) -> bool {
    fetched_at
        .parse::<jiff::Timestamp>()
        .is_ok_and(|at| (0..secs as i64).contains(&(now - at.as_second())))
}

/// The project's tasks `list` prints (see `tasks_for`).
pub fn tasks(
    ctx: &Ctx,
    project: &Project,
    config: &Config,
    fresh: bool,
) -> Result<Vec<Task>, String> {
    tasks_for(ctx, project, config, Scope::PROJECT, fresh)
}

/// The tasks `list` prints for `scope`: the cached answer while it is
/// younger than `cache_secs`, else a new one, which is cached whether or
/// not the tracker answered, so a tracker that is down is not asked every
/// turn either. `fresh` always asks. `Err` says why there are none, in this
/// binary's words.
pub fn tasks_for(
    ctx: &Ctx,
    project: &Project,
    config: &Config,
    scope: Scope,
    fresh: bool,
) -> Result<Vec<Task>, String> {
    let now = jiff::Timestamp::now().as_second();
    let cached = read_cache(project, config, scope)
        .filter(|c| !fresh && younger(&c.fetched_at, now, config.cache_secs));
    let answer = match cached {
        Some(cache) => cache.answer,
        None => {
            let env = [("HP_NODE_ID", scope.node), ("HP_TASK_SCOPE", scope.scope)];
            let answer = run(ctx, project, &config.list, config.timeout_secs, &env)
                .and_then(|stdout| parse(&stdout));
            let _ = project::write_json(
                &cache_path(project, scope.node),
                &Cache {
                    list: config.list.clone(),
                    scope: scope.scope.to_string(),
                    fetched_at: now_text(now),
                    answer: answer.clone(),
                },
            );
            answer
        }
    };
    tasks_of(answer)
}

/// The last answer, whatever its age, for the popup, which has no timer and
/// never runs `list`. `None` before the first.
pub fn cached(project: &Project, config: &Config) -> Option<Result<Vec<Task>, String>> {
    cached_for(project, config, Scope::PROJECT)
}

pub fn cached_for(
    project: &Project,
    config: &Config,
    scope: Scope,
) -> Option<Result<Vec<Task>, String>> {
    read_cache(project, config, scope).map(|cache| tasks_of(cache.answer))
}

/// When the project's cached list was asked for, RFC 3339; empty before the
/// first.
pub fn fetched_at(project: &Project) -> String {
    project::read_json::<Cache>(&cache_path(project, crate::organizations::ROOT_ID))
        .map(|cache| cache.fetched_at)
        .unwrap_or_default()
}

/// `.state/tracker-get.json`: per task id, the last answer of `get` and when.
type Gets = std::collections::BTreeMap<String, Got>;

#[derive(Serialize, Deserialize)]
struct Got {
    fetched_at: String,
    /// The task, `None` when the tracker has no task of that id.
    answer: Result<Option<Entry>, String>,
}

fn gets_path(project: &Project) -> std::path::PathBuf {
    project.state_dir().join("tracker-get.json")
}

/// The task whose id is `id`, as `get` prints it: cached for `cache_secs`
/// like a list. `Ok(None)` when the tracker has none of that id, and
/// always without a `get` command. Only a plain id is asked for.
pub fn get(
    ctx: &Ctx,
    project: &Project,
    config: &Config,
    id: &str,
) -> Result<Option<Task>, String> {
    if config.get.is_empty() || !is_plain_id(id) {
        return Ok(None);
    }
    let now = jiff::Timestamp::now().as_second();
    let mut gets: Gets = project::read_json(&gets_path(project)).unwrap_or_default();
    if let Some(got) = gets.get(id)
        && younger(&got.fetched_at, now, config.cache_secs)
    {
        return got
            .answer
            .clone()
            .map(|e| e.as_ref().map(Entry::task))
            .map_err(unavailable);
    }
    let answer = run(
        ctx,
        project,
        &config.get,
        config.timeout_secs,
        &[("HP_TASK_ID", id)],
    )
    .and_then(|stdout| parse_one(&stdout));
    gets.insert(
        id.to_string(),
        Got {
            fetched_at: now_text(now),
            answer: answer.clone(),
        },
    );
    let _ = project::write_json(&gets_path(project), &gets);
    answer
        .map(|e| e.as_ref().map(Entry::task))
        .map_err(unavailable)
}

/// Every task `get` found, whatever its age: what a reader that runs
/// nothing (asks, the popup) knows besides the lists.
pub fn cached_gets(project: &Project) -> Vec<Task> {
    project::read_json::<Gets>(&gets_path(project))
        .unwrap_or_default()
        .values()
        .filter_map(|got| got.answer.as_ref().ok()?.as_ref().map(Entry::task))
        .collect()
}

/// Every list cached for a coordinator node's task scope, whatever its age.
pub fn cached_scopes(project: &Project, config: &Config) -> Vec<Task> {
    let Ok(entries) = std::fs::read_dir(project.state_dir()) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|name| name.starts_with("tracker.t-") && name.ends_with(".json"))
        .filter_map(|name| project::read_json::<Cache>(&project.state_dir().join(name)))
        .filter(|cache| cache.list == config.list)
        .filter_map(|cache| cache.answer.ok())
        .flatten()
        .map(|entry| entry.task())
        .collect()
}

fn unavailable(reason: String) -> String {
    format!("tracker unavailable: its get command {reason}")
}

/// The task in `get`'s output, or why it is not one.
fn parse_one(stdout: &str) -> Result<Option<Entry>, String> {
    #[derive(Deserialize)]
    struct Document {
        schema_version: u32,
        task: Option<Entry>,
    }
    let not_a_task = || "printed no task (schema_version 1)".to_string();
    let document: Document = serde_json::from_str(stdout).map_err(|_| not_a_task())?;
    if document.schema_version != 1 {
        return Err(not_a_task());
    }
    match document.task {
        Some(e) if !is_plain_id(e.id.trim()) => {
            Err("printed a task whose id is not a plain identifier".into())
        }
        Some(e) if one_line(Some(&e.title)).is_empty() => {
            Err("printed a task without an id or a title".into())
        }
        task => Ok(task),
    }
}

fn tasks_of(answer: Result<Vec<Entry>, String>) -> Result<Vec<Task>, String> {
    answer
        .map(|entries| entries.iter().map(Entry::task).collect())
        .map_err(|reason| format!("tracker unavailable: its list command {reason}"))
}

/// The tasks in `list`'s output, or why it is not a task list.
fn parse(stdout: &str) -> Result<Vec<Entry>, String> {
    #[derive(Deserialize)]
    struct Document {
        schema_version: u32,
        tasks: Vec<Entry>,
    }
    let not_a_list = || "printed no task list (schema_version 1)".to_string();
    let document: Document = serde_json::from_str(stdout).map_err(|_| not_a_list())?;
    if document.schema_version != 1 {
        return Err(not_a_list());
    }
    if document.tasks.iter().any(|e| !is_plain_id(e.id.trim())) {
        return Err("printed a task whose id is not a plain identifier".into());
    }
    if document
        .tasks
        .iter()
        .any(|e| one_line(Some(&e.title)).is_empty())
    {
        return Err("printed a task without an id or a title".into());
    }
    Ok(document.tasks)
}

/// An id that is safe in a prompt and on a command line: 1 to 64 letters,
/// digits, `.`, `_`, `:`, `/` and `-`, not starting with `-`.
fn is_plain_id(id: &str) -> bool {
    (1..=64).contains(&id.len())
        && !id.starts_with('-')
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._:/-".contains(c))
}

/// A config.toml error about the tracker table, as the one line `context`
/// and the popup print in place of the tasks.
pub fn config_error(error: &anyhow::Error) -> String {
    format!(
        "tracker unavailable: {}",
        one_line(Some(&format!("{error:#}")))
    )
}

/// Runs one of the tracker's commands; its standard output, or why it failed.
/// Nothing it printed is in the reason.
fn run(
    ctx: &Ctx,
    project: &Project,
    argv: &[String],
    timeout_secs: u64,
    env: &[(&str, &str)],
) -> Result<String, String> {
    let Some((program, args)) = argv.split_first() else {
        return Err("is empty".into());
    };
    let mut cmd = Cmd::new(program, Duration::from_secs(timeout_secs))
        .args(args)
        .cwd(project.dir())
        .env("HP_PROJECT", &project.slug)
        .own_group();
    for (key, value) in env {
        cmd = cmd.env(*key, *value);
    }
    match ctx.runner.run(&cmd) {
        Err(_) => Err("could not be started".into()),
        Ok(out) if out.timed_out => Err(format!("timed out after {timeout_secs} s")),
        Ok(out) => match out.code {
            Some(0) => Ok(out.stdout),
            Some(code) => Err(format!("exited with status {code}")),
            None => Err("was stopped by a signal".into()),
        },
    }
}

/// Tells the tracker's `link` about `event` on the task thread `t` was
/// started from. It never fails and never undoes anything: a `link` that does
/// not succeed leaves a `task` item in the inbox. The cached list is dropped,
/// since the tracker may have changed.
pub fn link(ctx: &Ctx, project: &Project, t: &Thread, event: Event) {
    let assigned = t.assigned();
    if assigned.is_empty() {
        return;
    }
    let Ok(Some(config)) = project.tracker(&ctx.config_dir) else {
        return;
    };
    if config.link.is_empty() {
        return;
    }
    for task_id in assigned {
        link_one(ctx, project, &config, t, task_id, event);
    }
}

/// `link` for one of the tasks `t` works on.
fn link_one(
    ctx: &Ctx,
    project: &Project,
    config: &Config,
    t: &Thread,
    task_id: &str,
    event: Event,
) {
    let mut env = vec![
        ("HP_TASK_ID", task_id),
        ("HP_THREAD_ID", t.id.as_str()),
        ("HP_EVENT", event.name()),
    ];
    if !t.pr.is_empty() {
        env.push(("HP_PR_URL", t.pr.as_str()));
    }
    let result = run(ctx, project, &config.link, config.timeout_secs, &env);
    let _ = std::fs::remove_file(cache_path(project, crate::organizations::ROOT_ID));
    if let Err(reason) = result {
        let _ = crate::inbox::write(
            project,
            "task",
            &t.id,
            "tracker link failed",
            &format!(
                "the tracker's link command failed for task {} ({}): it {reason}",
                task_id,
                event.name()
            ),
            "",
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_list_needs_its_schema_version_and_an_id_and_title_per_task() {
        let tasks = parse(
            r#"{"schema_version":1,"tasks":[{"id":" TRK-1 ","title":"Fix\tlogin","owner":"claude@m1","thread":null,"description":"a\u0007b\nc\n","extra":1}]}"#,
        )
        .unwrap();
        let task = tasks[0].task();
        assert_eq!(
            (
                task.id.as_str(),
                task.title.as_str(),
                task.description.as_str()
            ),
            ("TRK-1", "Fix login", "ab\nc")
        );
        assert_eq!(
            task.owner,
            Owner::Agent {
                profile: Some("claude".into()),
                machine: Some("m1".into())
            }
        );
        assert_eq!((task.thread, task.line, task.done), (None, 0, false));
        for bad in [
            "Error: not signed in",
            r#"{"schema_version":2,"tasks":[]}"#,
            r#"{"tasks":[]}"#,
        ] {
            assert_eq!(
                parse(bad).unwrap_err(),
                "printed no task list (schema_version 1)"
            );
        }
        assert_eq!(
            parse(r#"{"schema_version":1,"tasks":[{"id":"x","title":" "}]}"#).unwrap_err(),
            "printed a task without an id or a title"
        );
        // An id reaches a prompt and a command line: a plain identifier only.
        let long = "x".repeat(65);
        for bad in ["", "TRK 1", "TRK\\\"1", "-rf", "#12", "a;b", long.as_str()] {
            let json =
                serde_json::json!({"schema_version": 1, "tasks": [{"id": bad, "title": "x"}]});
            assert_eq!(
                parse(&json.to_string()).unwrap_err(),
                "printed a task whose id is not a plain identifier",
                "{bad}"
            );
        }
        for good in ["TRK-1", "owner/repo:12", "a.b_c", &"x".repeat(64)] {
            let json =
                serde_json::json!({"schema_version": 1, "tasks": [{"id": good, "title": "x"}]});
            assert!(parse(&json.to_string()).is_ok(), "{good}");
        }
    }
}

//! A project's tasks from a tracker outside it, through two commands the user
//! names in config.toml (`[tracker."<canonical project folder>"]`): `list`
//! prints the tasks as JSON, and `link` hears when a thread started from one
//! of them starts, is resolved or merges. Both are argv run in the project
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

/// `.state/tracker.json`: the last answer of `list` and when it came.
#[derive(Serialize, Deserialize)]
struct Cache {
    /// The command it came from; another one is not this cache.
    list: Vec<String>,
    fetched_at: String,
    /// The tasks, or why there are none.
    answer: Result<Vec<Entry>, String>,
}

fn cache_path(project: &Project) -> std::path::PathBuf {
    project.state_dir().join("tracker.json")
}

fn read_cache(project: &Project, config: &Config) -> Option<Cache> {
    project::read_json::<Cache>(&cache_path(project)).filter(|c| c.list == config.list)
}

/// The tasks `list` prints: the cached answer while it is younger than
/// `cache_secs`, else a new one, which is cached whether or not the tracker
/// answered, so a tracker that is down is not asked every turn either.
/// `fresh` always asks. `Err` says why there are none, in this binary's words.
pub fn tasks(
    ctx: &Ctx,
    project: &Project,
    config: &Config,
    fresh: bool,
) -> Result<Vec<Task>, String> {
    let now = jiff::Timestamp::now();
    let cached = read_cache(project, config).filter(|c| {
        !fresh
            && c.fetched_at.parse::<jiff::Timestamp>().is_ok_and(|at| {
                at <= now && now.as_second() - at.as_second() < config.cache_secs as i64
            })
    });
    let answer = match cached {
        Some(cache) => cache.answer,
        None => {
            let answer = run(ctx, project, &config.list, config.timeout_secs, &[])
                .and_then(|stdout| parse(&stdout));
            let _ = project::write_json(
                &cache_path(project),
                &Cache {
                    list: config.list.clone(),
                    fetched_at: now.to_string(),
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
    read_cache(project, config).map(|cache| tasks_of(cache.answer))
}

/// When the cached list was asked for, RFC 3339; empty before the first.
pub fn fetched_at(project: &Project) -> String {
    project::read_json::<Cache>(&cache_path(project))
        .map(|cache| cache.fetched_at)
        .unwrap_or_default()
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
    if document
        .tasks
        .iter()
        .any(|e| one_line(Some(&e.id)).is_empty() || one_line(Some(&e.title)).is_empty())
    {
        return Err("printed a task without an id or a title".into());
    }
    Ok(document.tasks)
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
    if t.task_id.is_empty() {
        return;
    }
    let Ok(Some(config)) = project.tracker(&ctx.config_dir) else {
        return;
    };
    if config.link.is_empty() {
        return;
    }
    let mut env = vec![
        ("HP_TASK_ID", t.task_id.as_str()),
        ("HP_THREAD_ID", t.id.as_str()),
        ("HP_EVENT", event.name()),
    ];
    if !t.pr.is_empty() {
        env.push(("HP_PR_URL", t.pr.as_str()));
    }
    let result = run(ctx, project, &config.link, config.timeout_secs, &env);
    let _ = std::fs::remove_file(cache_path(project));
    if let Err(reason) = result {
        let _ = crate::inbox::write(
            project,
            "task",
            &t.id,
            "tracker link failed",
            &format!(
                "the tracker's link command failed for task {} ({}): it {reason}",
                t.task_id,
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
            r#"{"schema_version":1,"tasks":[{"id":" AWA-1 ","title":"Fix\tlogin","owner":"claude@m1","thread":null,"description":"a\u0007b\nc\n","extra":1}]}"#,
        )
        .unwrap();
        let task = tasks[0].task();
        assert_eq!(
            (
                task.id.as_str(),
                task.title.as_str(),
                task.description.as_str()
            ),
            ("AWA-1", "Fix login", "ab\nc")
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
            parse(r#"{"schema_version":1,"tasks":[{"id":"","title":"x"}]}"#).unwrap_err(),
            "printed a task without an id or a title"
        );
    }
}

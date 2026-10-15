//! `project status [--json]`: the project's work in one document, computed on
//! every call from TASKS.md, HANDOFF.md, the thread records with their pull
//! requests, and the inbox. Nobody writes it, so it cannot drift from them the
//! way a hand-kept notes file does.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::Result;
use serde::Serialize;

use crate::drift::{self, Problem, Rule};
use crate::paths::Ctx;
use crate::project::Project;
use crate::tasks::{self, Task};
use crate::thread::{Status, Thread};
use crate::threads::{self, PrJson, ThreadJson};

/// One row of the status: a TASKS.md task, or an open thread no task names.
#[derive(Serialize)]
struct Item {
    kind: &'static str,
    id: String,
    title: String,
    list: String,
    /// A task's owner as `task list --json` shows it; `null` for a thread.
    owner: serde_json::Value,
    state: &'static str,
    group: &'static str,
    task: String,
    thread: String,
    pr: Option<PrJson>,
    created: String,
    updated: String,
    flags: Vec<&'static str>,
}

/// A thread's group as one of the status states.
fn state_of(group: &str) -> &'static str {
    match group {
        "waiting-on-you" => "waiting",
        "ready-for-review" => "review",
        "landing" => "landing",
        "working" => "working",
        "idle" => "idle",
        _ => "done",
    }
}

/// Its last agent state change, else its last record write.
fn updated(t: &Thread) -> String {
    if t.last_state_change.is_empty() {
        t.updated.clone()
    } else {
        t.last_state_change.clone()
    }
}

fn task_item(
    task: &Task,
    owner: serde_json::Value,
    thread: Option<&(Thread, ThreadJson)>,
    problems: &[Problem],
) -> Item {
    let mut flags: Vec<&'static str> = problems
        .iter()
        .filter(|p| p.line == task.line)
        .map(|p| p.rule.name())
        .collect();
    let (state, group, pr, created, updated) = match thread {
        Some((t, json)) => {
            if t.pr_merged() {
                flags.push("pr-merged");
            }
            // A task left on a resolved thread is done once its work merged,
            // else it is waiting to be taken up again.
            let state = match (task.done, t.status, t.pr_merged()) {
                (true, _, _) => "done",
                (false, Status::Resolved, true) => "done",
                (false, Status::Resolved, false) => "todo",
                _ => state_of(json.group),
            };
            (
                state,
                json.group,
                json.pr.clone(),
                t.created.clone(),
                updated(t),
            )
        }
        None => (
            if task.done { "done" } else { "todo" },
            "",
            None,
            String::new(),
            String::new(),
        ),
    };
    Item {
        kind: "task",
        id: format!("task:{}", task.line),
        title: task.title.clone(),
        list: task.list.clone(),
        owner,
        state,
        group,
        task: task.title.clone(),
        thread: task.thread.clone().unwrap_or_default(),
        pr,
        created,
        updated,
        flags,
    }
}

fn thread_item(t: &Thread, json: &ThreadJson) -> Item {
    Item {
        kind: "thread",
        id: t.id.clone(),
        title: t.title.clone(),
        list: String::new(),
        owner: serde_json::Value::Null,
        state: state_of(json.group),
        group: json.group,
        task: t.task.clone(),
        thread: t.id.clone(),
        pr: json.pr.clone(),
        created: t.created.clone(),
        updated: updated(t),
        flags: if t.pr_merged() {
            vec!["pr-merged"]
        } else {
            Vec::new()
        },
    }
}

/// The first paragraph under `## Current objective` in HANDOFF.md, its lines
/// joined by spaces.
fn objective(handoff: &str) -> String {
    handoff
        .lines()
        .skip_while(|l| !l.trim().eq_ignore_ascii_case("## current objective"))
        .skip(1)
        .skip_while(|l| l.trim().is_empty())
        .take_while(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .map(str::trim)
        .collect::<Vec<_>>()
        .join(" ")
}

/// A file's modification time, RFC 3339 to the second; empty when missing.
fn modified(path: &Path) -> String {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| jiff::Timestamp::try_from(t).ok())
        .and_then(|t| t.round(jiff::Unit::Second).ok())
        .map(|t| t.to_string())
        .unwrap_or_default()
}

pub fn print(ctx: &Ctx, slug: &str, json: bool) -> Result<()> {
    let project = Project::load(&ctx.root, slug)?;
    let dir = project.dir();
    let rows = threads::rows(ctx, &project);
    let threads: BTreeMap<String, (Thread, ThreadJson)> = rows
        .iter()
        .map(|row| row.thread.clone())
        .zip(threads::threads_json(&project, &rows))
        .map(|(t, json)| (t.id.clone(), (t, json)))
        .collect();
    let list = tasks::parse(&tasks::read(&dir));
    let problems = drift::problems(&project);
    let config = crate::profiles::load(&ctx.config_dir)?;
    let is_profile = |p: &str| config.get(p).is_some();

    let mut items: Vec<Item> = list
        .iter()
        .map(|task| {
            let owner = tasks::to_json(task, is_profile)["owner"].take();
            let thread = task.thread.as_deref().and_then(|id| threads.get(id));
            task_item(task, owner, thread, &problems)
        })
        .collect();
    let named: Vec<&str> = list.iter().filter_map(|t| t.thread.as_deref()).collect();
    items.extend(
        threads
            .values()
            .filter(|(t, _)| t.status != Status::Resolved && !named.contains(&t.id.as_str()))
            .map(|(t, json)| thread_item(t, json)),
    );

    if !json {
        for item in &items {
            println!(
                "{}\t{}\t{}\t{}",
                item.state,
                item.id,
                item.title,
                item.flags.join(",")
            );
        }
        for problem in &problems {
            println!("{}", problem.text());
        }
        return Ok(());
    }

    let mut drift: BTreeMap<String, usize> = Rule::ALL
        .iter()
        .map(|rule| {
            (
                rule.name().replace('-', "_"),
                problems.iter().filter(|p| p.rule == *rule).count(),
            )
        })
        .collect();
    drift.insert(
        "inbox_unhandled".into(),
        crate::inbox::unhandled(&project).len(),
    );
    let next: Vec<serde_json::Value> = threads
        .values()
        .filter(|(t, _)| t.status != Status::Resolved)
        .flat_map(|(t, json)| {
            json.next
                .iter()
                .map(|line| serde_json::json!({ "thread": t.id, "line": line }))
        })
        .collect();
    let handoff = std::fs::read_to_string(dir.join("HANDOFF.md")).unwrap_or_default();
    threads::print_json(&serde_json::json!({
        "schema_version": threads::JSON_SCHEMA_VERSION,
        "project": project.slug,
        "generated_at": crate::project::now(),
        "objective": objective(&handoff),
        "sources": {
            "tasks_updated": modified(&dir.join("TASKS.md")),
            "handoff_updated": modified(&dir.join("HANDOFF.md")),
            "handoff_chars": handoff.chars().count(),
            "handoff_limit": drift::MAX_HANDOFF_CHARS,
            "pr_checked": crate::steps::load_state(&project).last_pr_check,
        },
        "items": items,
        "drift": drift,
        "problems": problems.iter().map(Problem::to_json).collect::<Vec<_>>(),
        "next": next,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_objective_is_the_first_paragraph_of_its_section() {
        assert_eq!(
            objective("# H\n\n## Current objective\n\nShip it\n  today.\n\nLater.\n## Next\n"),
            "Ship it today."
        );
        assert_eq!(
            objective("## Current Objective\nOne\n## Active work\n"),
            "One"
        );
        assert_eq!(objective("# H\n\nNo section.\n"), "");
    }
}

//! Drift: where the tasks (TASKS.md, or a tracker's) and the root's STATUS.md
//! disagree with the thread records and the user's messages.
//! `context` lists it for the coordinator every turn. It is computed from the
//! files on each call; nothing is stored, and nothing here changes a file.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use crate::paths::Ctx;
use crate::project::Project;
use crate::status_file::{self, MAX_CHARS};
use crate::tasks::{self, Owner, Task, Tasks};
use crate::thread::{self, Status, Thread};

/// Lines of the drift section, the "more" line included.
const MAX_LINES: usize = 10;

/// In the order `context` lists them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Rule {
    StatusOverLimit,
    StatusStale,
    /// An ask of the root's status file that does not close (`crate::asks`).
    /// Only `context` lists it: `task lint` and `project status` keep to
    /// the rules of `ALL`.
    AskMismatch,
    ThreadResolved,
    ThreadMissing,
    DoneNotDeleted,
    StatusInOwner,
    DuplicateTitle,
}

impl Rule {
    /// What `problems` checks; `ask-mismatch` is `context_problems`' own.
    pub const ALL: [Rule; 7] = [
        Rule::StatusOverLimit,
        Rule::StatusStale,
        Rule::ThreadResolved,
        Rule::ThreadMissing,
        Rule::DoneNotDeleted,
        Rule::StatusInOwner,
        Rule::DuplicateTitle,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Rule::StatusOverLimit => "status-over-limit",
            Rule::StatusStale => "status-stale",
            Rule::AskMismatch => "ask-mismatch",
            Rule::ThreadResolved => "thread-resolved",
            Rule::ThreadMissing => "thread-missing",
            Rule::DoneNotDeleted => "done-not-deleted",
            Rule::StatusInOwner => "status-in-owner",
            Rule::DuplicateTitle => "duplicate-title",
        }
    }

    /// About the root's status file rather than a TASKS.md line.
    pub fn about_status(self) -> bool {
        matches!(
            self,
            Rule::StatusOverLimit | Rule::StatusStale | Rule::AskMismatch
        )
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Problem {
    pub rule: Rule,
    /// The TASKS.md line, from 1; 0 for the status file and a tracker task;
    /// the status file's line for `ask-mismatch`.
    pub line: usize,
    /// A tracker task's id, empty otherwise.
    pub id: String,
    /// The task's title, empty for the status file.
    pub task: String,
    /// The thread the task line names, empty when none.
    pub thread: String,
    /// What is wrong, in a few words of this binary's.
    pub detail: String,
}

impl Problem {
    /// The `task lint --json` and `project status --json` shape; `id` only
    /// for a tracker task.
    pub fn to_json(&self) -> serde_json::Value {
        let mut json = serde_json::json!({
            "rule": self.rule.name(),
            "line": self.line,
            "task": self.task,
            "thread": self.thread,
            "detail": self.detail,
        });
        if !self.id.is_empty() {
            json["id"] = self.id.clone().into();
        }
        json
    }

    /// One line of the digest's drift section.
    pub fn text(&self) -> String {
        if self.rule.about_status() {
            return format!("- [{}] {}", self.rule.name(), self.detail);
        }
        let thread = if self.thread.is_empty() {
            String::new()
        } else {
            format!(" · {}", one_line(&self.thread))
        };
        let place = if self.id.is_empty() {
            format!("TASKS.md:{}", self.line)
        } else {
            format!("tracker {}", one_line(&self.id))
        };
        format!(
            "- [{}] {place} \"{}\"{thread}: {}",
            self.rule.name(),
            one_line(&self.task),
            self.detail
        )
    }

    /// Whether it is about `task` rather than the status file or another task.
    pub fn is_about(&self, task: &Task) -> bool {
        !self.rule.about_status() && self.line == task.line && self.id == task.id
    }
}

fn one_line(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

/// The length in characters of the root's status file when it is over what
/// `context` prints, `None` within it.
pub fn status_over(project: &Project) -> Option<usize> {
    let chars = status_file::root_text(project).chars().count();
    (chars > MAX_CHARS).then_some(chars)
}

/// The root's status file problems: too long, or older than the user's
/// previous message.
fn status_problems(project: &Project) -> Vec<Problem> {
    let problem = |rule: Rule, detail: String| Problem {
        rule,
        line: 0,
        id: String::new(),
        task: String::new(),
        thread: String::new(),
        detail,
    };
    let name = status_file::NAME;
    let mut found = Vec::new();
    if let Some(chars) = status_over(project) {
        found.push(problem(
            Rule::StatusOverLimit,
            format!("{name} has {chars} characters, this digest shows the first {MAX_CHARS}"),
        ));
    }
    let previous = status_file::previous_ask(project, status_file::ROOT_KEY);
    if status_file::stale(&status_file::root(project), previous) {
        found.push(problem(
            Rule::StatusStale,
            format!("{name} is older than the user's last message"),
        ));
    }
    found
}

/// Every problem in the project's files and `tasks`, in listing order. A
/// tracker's tasks are checked only against the thread records: the format
/// rules are about TASKS.md lines. An unavailable tracker has none.
pub fn problems(project: &Project, tasks: &Tasks) -> Vec<Problem> {
    let mut found = status_problems(project);
    let records: BTreeMap<String, Thread> = thread::list(project)
        .into_iter()
        .map(|t| (t.id.clone(), t))
        .collect();
    match tasks {
        Tasks::File(list) => found.extend(task_problems(list, &records)),
        Tasks::Tracker(list) => {
            for task in list {
                found.extend(thread_problem(task, &records));
            }
        }
        Tasks::Unavailable(_) => {}
    }
    found.sort_by_key(|p| (p.rule, p.line));
    found
}

/// What `context` lists: `problems`, and the root's asks that do not close,
/// each named by its line and the binary's reason, never by its text.
pub fn context_problems(project: &Project, tasks: &Tasks) -> Vec<Problem> {
    let mut found = problems(project, tasks);
    let records = crate::asks::Records::new(thread::list(project), tasks.list().to_vec());
    let asks = status_file::read(&status_file::root(project))
        .map(|status| {
            status
                .confirmed(&records, crate::organizations::ROOT_ID)
                .asks
        })
        .unwrap_or_default();
    found.extend(
        asks.iter()
            .filter(|ask| !ask.mismatch.is_empty())
            .map(|ask| Problem {
                rule: Rule::AskMismatch,
                line: ask.line,
                id: String::new(),
                task: String::new(),
                thread: String::new(),
                detail: format!(
                    "{}:{} is marked {}, but {}",
                    status_file::NAME,
                    ask.line,
                    ask.state.word(),
                    ask.mismatch
                ),
            }),
    );
    found.sort_by_key(|p| (p.rule, p.line));
    found
}

fn problem(rule: Rule, task: &Task, detail: String) -> Problem {
    Problem {
        rule,
        line: task.line,
        id: task.id.clone(),
        task: task.title.clone(),
        thread: task.thread.clone().unwrap_or_default(),
        detail,
    }
}

/// A task that names a thread with no record, or an open task on a resolved
/// thread.
fn thread_problem(task: &Task, records: &BTreeMap<String, Thread>) -> Option<Problem> {
    match task.thread.as_deref().map(|id| records.get(id)) {
        Some(None) => Some(problem(Rule::ThreadMissing, task, "no such thread".into())),
        Some(Some(t)) if !task.done && t.status == Status::Resolved => {
            let merged = if t.pr_merged() {
                " and its PR merged"
            } else {
                ""
            };
            Some(problem(
                Rule::ThreadResolved,
                task,
                format!("the thread is resolved{merged}"),
            ))
        }
        _ => None,
    }
}

fn task_problems(list: &[Task], records: &BTreeMap<String, Thread>) -> Vec<Problem> {
    let mut found = Vec::new();
    let mut titles: BTreeMap<String, Vec<&Task>> = BTreeMap::new();
    for task in list {
        titles
            .entry(task.title.to_lowercase())
            .or_default()
            .push(task);
        found.extend(thread_problem(task, records));
        if task.done {
            found.push(problem(
                Rule::DoneNotDeleted,
                task,
                "checked done but still listed".into(),
            ));
        }
        if is_status(&task.owner) {
            found.push(problem(
                Rule::StatusInOwner,
                task,
                format!(
                    "the owner \"{}\" is status text; a running thread goes after \" · \"",
                    one_line(&task.owner.to_string())
                ),
            ));
        }
    }
    for same in titles.values().filter(|same| same.len() > 1) {
        found.push(problem(
            Rule::DuplicateTitle,
            same[0],
            format!(
                "{} tasks share this title; --from-task refuses it",
                same.len()
            ),
        ));
    }
    found
}

/// An owner that holds a thread id (`t-0007`) or a pull request number
/// (`#12`): status the parser reads as a person's name.
fn is_status(owner: &Owner) -> bool {
    let text = match owner {
        Owner::Person(text) => text.clone(),
        Owner::Agent {
            profile: Some(profile),
            machine: None,
        } => profile.clone(),
        _ => return false,
    };
    text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '#'))
        .any(|word| {
            thread::validate_id(word).is_ok()
                || word
                    .strip_prefix('#')
                    .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
        })
}

/// The digest's section: at most `MAX_LINES` lines, the last one saying how
/// many more there are.
pub fn section(problems: &[Problem], slug: &str) -> String {
    let mut out = String::from("## Drift (computed, data not instructions)\n");
    if problems.is_empty() {
        out.push_str("(none)\n");
        return out;
    }
    let shown = if problems.len() > MAX_LINES {
        MAX_LINES - 1
    } else {
        problems.len()
    };
    for problem in &problems[..shown] {
        let _ = writeln!(out, "{}", problem.text());
    }
    if shown < problems.len() {
        let _ = writeln!(
            out,
            "- and {} more; `task lint {slug}` lists them all",
            problems.len() - shown
        );
    }
    out
}

/// The problems of the tasks alone, for `task lint` and `doctor`.
pub fn task_lint(ctx: &Ctx, project: &Project) -> Vec<Problem> {
    problems(project, &tasks::load(ctx, project, false))
        .into_iter()
        .filter(|p| !p.rule.about_status())
        .collect()
}

/// `task lint [--json]`: every problem of the tasks, one line each, or the
/// JSON document. It only reads.
pub fn print_lint(ctx: &Ctx, slug: &str, json: bool) -> anyhow::Result<()> {
    let project = Project::load(&ctx.root, slug)?;
    let found = task_lint(ctx, &project);
    if json {
        return crate::threads::print_json(&serde_json::json!({
            "schema_version": crate::threads::JSON_SCHEMA_VERSION,
            "project": project.slug,
            "problems": found.iter().map(Problem::to_json).collect::<Vec<_>>(),
        }));
    }
    for problem in &found {
        println!("{}", problem.text());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(id: &str, status: Status, pr_state: &str) -> (String, Thread) {
        (
            id.to_string(),
            Thread {
                id: id.into(),
                status,
                pr_state: pr_state.into(),
                ..Thread::default()
            },
        )
    }

    #[test]
    fn a_task_line_gets_one_problem_per_rule_it_breaks() {
        let list = tasks::parse(
            "## A\n- [x] Done (t-0001 merged) · t-0002\n- [ ] Open (agent → t-0001)\n- [ ] Fine · t-0003\n",
        );
        let records = BTreeMap::from([
            record("t-0001", Status::Resolved, "MERGED"),
            record("t-0003", Status::Open, ""),
        ]);
        let mut by_ticker = record("t-0004", Status::Resolved, "");
        by_ticker.1.resolved_reason = "merged".into();
        let merged = task_problems(
            &tasks::parse("- [ ] Shipped · t-0004\n"),
            &BTreeMap::from([by_ticker]),
        );
        assert_eq!(merged[0].detail, "the thread is resolved and its PR merged");
        let mut found = task_problems(&list, &records);
        found.sort_by_key(|p| (p.rule, p.line));
        let rules: Vec<(Rule, usize)> = found.iter().map(|p| (p.rule, p.line)).collect();
        assert_eq!(
            rules,
            [
                (Rule::ThreadResolved, 3),
                (Rule::ThreadMissing, 2),
                (Rule::DoneNotDeleted, 2),
                (Rule::StatusInOwner, 2),
            ]
        );
        assert_eq!(
            found[0].text(),
            "- [thread-resolved] TASKS.md:3 \"Open\" · t-0001: the thread is resolved and its PR merged"
        );
    }

    #[test]
    fn status_in_an_owner_is_a_thread_id_or_a_pull_request_number() {
        for owner in ["t-0007", "t-0007 merged", "PR #12", "done, see #3"] {
            assert!(is_status(&Owner::parse(owner)), "{owner}");
        }
        for owner in ["claude", "Priya Rao", "me", "", "t-7", "#", "codex@t-0007"] {
            assert!(!is_status(&Owner::parse(owner)), "{owner}");
        }
    }
}

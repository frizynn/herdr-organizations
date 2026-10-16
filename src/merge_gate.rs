//! A project's own merge check. `merge_gate` in its safety table names a
//! command that decides, in place of GitHub's approval and "no checks" rule,
//! whether a thread's pull request may merge. It runs only inside
//! `steps::merge_pull_request`, just before `gh pr merge`, on the head commit
//! the merge is then pinned to. Its output is the user's script's text: only
//! a clean first line is kept, for the inbox, `--json` and the popup. It never
//! reaches a prompt.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::paths::Ctx;
use crate::pr;
use crate::project::{self, Project};
use crate::runner::{Cmd, Runner};
use crate::thread::Thread;

/// At most this many characters of a refusal's first line are kept.
const REASON_LIMIT: usize = 200;

/// The gate's last answer for one thread.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(default)]
pub struct Verdict {
    /// The head commit it ran on.
    pub head_oid: String,
    pub passed: bool,
    /// Why it refused; empty when it passed.
    pub reason: String,
}

/// A merge the gate refused. Its inbox item is already written, once per
/// reason, so the ticker writes no item of its own.
#[derive(Debug)]
pub struct Refused {
    pub id: String,
    pub reason: String,
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}: not merged by the merge gate: {}",
            self.id, self.reason
        )
    }
}

impl std::error::Error for Refused {}

/// `.state/merge-gate.json`: thread id -> the gate's last verdict. `thread
/// merge`, the popup and the ticker all write it, so every change is a
/// read-modify-write under the project lock.
fn path(project: &Project) -> PathBuf {
    project.state_dir().join("merge-gate.json")
}

pub fn load(project: &Project) -> BTreeMap<String, Verdict> {
    project::read_json(&path(project)).unwrap_or_default()
}

/// Records `verdict` as thread `id`'s last.
pub fn record(project: &Project, id: &str, verdict: &Verdict) -> Result<()> {
    let _lock = project.lock()?;
    let mut all = load(project);
    all.insert(id.to_string(), verdict.clone());
    project::write_json(&path(project), &all)
}

/// Runs `gate` (program and arguments, no shell) on `summary`'s head commit,
/// without standard input, in the thread's local repository, with the pull
/// request in `HP_*` variables. Exit 0 passes; any other exit, a timeout or a
/// command that cannot start refuses.
pub fn run(
    runner: &dyn Runner,
    project: &Project,
    t: &Thread,
    summary: &pr::Summary,
    gate: &[String],
    timeout: Duration,
) -> Verdict {
    let verdict = |passed: bool, reason: String| Verdict {
        head_oid: summary.head_oid.clone(),
        passed,
        reason,
    };
    let Some((program, args)) = gate.split_first() else {
        return verdict(false, "merge_gate is empty".into());
    };
    let url = t.pr.trim_start_matches("https://github.com/");
    let parts: Vec<&str> = url.split('/').collect();
    let local = |dir: &str| !t.is_remote() && !dir.is_empty() && Path::new(dir).is_dir();
    let mut cmd = Cmd::new(program.as_str(), timeout)
        .args(args)
        .own_group()
        .cwd(if local(&t.repo) {
            PathBuf::from(&t.repo)
        } else {
            project.dir()
        })
        .env("HP_PROJECT", &project.slug)
        .env("HP_THREAD_ID", &t.id)
        .env("HP_PR_URL", &t.pr)
        .env("HP_PR_NUMBER", parts.last().copied().unwrap_or(""))
        .env(
            "HP_REPO",
            parts.iter().take(2).copied().collect::<Vec<_>>().join("/"),
        )
        .env("HP_HEAD_SHA", &summary.head_oid)
        .env("HP_HEAD_REF", &t.branch)
        .env("HP_BASE_REF", &summary.base_ref);
    if local(&t.worktree_path) {
        cmd = cmd.env("HP_WORKTREE", &t.worktree_path);
    }
    match runner.run(&cmd) {
        Err(error) => verdict(false, pr::clean(&format!("{error:#}"), REASON_LIMIT)),
        Ok(out) if out.success() => verdict(true, String::new()),
        Ok(out) if out.timed_out => verdict(false, "merge gate timed out".into()),
        Ok(out) => {
            let first = pr::clean(out.stdout.lines().next().unwrap_or(""), REASON_LIMIT);
            let reason = if !first.is_empty() {
                first
            } else if let Some(code) = out.code {
                format!("merge gate exited {code}")
            } else {
                "merge gate was killed".into()
            };
            verdict(false, reason)
        }
    }
}

/// A project's merge policy as the views read it, once per project: GitHub's,
/// or a gate with its last verdicts. A config.toml that does not parse reads
/// as no gate here; a merge reads it again and refuses.
#[derive(Debug, Clone, Default)]
pub struct Rule {
    pub policy: pr::Policy,
    pub method: pr::MergeMethod,
    verdicts: BTreeMap<String, Verdict>,
    /// thread id -> its pull request's head commit as the ticker last read it.
    heads: BTreeMap<String, String>,
}

impl Rule {
    pub fn load(ctx: &Ctx, project: &Project) -> Rule {
        let Ok(safety) = project.safety(&ctx.config_dir) else {
            return Rule::default();
        };
        let mut rule = Rule {
            policy: safety.merge_policy(),
            method: safety.merge_method,
            ..Rule::default()
        };
        if rule.policy == pr::Policy::Gate {
            rule.verdicts = load(project);
            rule.heads = crate::steps::load_state(project)
                .prs
                .into_iter()
                .map(|(id, summary)| (id, summary.head_oid))
                .collect();
        }
        rule
    }

    /// For the Landing group: whether the gate passed thread `id` on its
    /// pull request's last read head commit; `None` under GitHub's policy,
    /// where the approval tells instead.
    pub fn landing(&self, id: &str) -> Option<bool> {
        self.passed(id, self.heads.get(id).map_or("", String::as_str))
    }

    /// The gate's verdict for thread `id` when it ran on commit `head`.
    pub fn verdict(&self, id: &str, head: &str) -> Option<&Verdict> {
        self.verdicts
            .get(id)
            .filter(|v| !head.is_empty() && v.head_oid == head)
    }

    /// Why the gate refused thread `id` on commit `head`, when it did.
    pub fn refusal(&self, id: &str, head: &str) -> Option<String> {
        self.verdict(id, head)
            .filter(|v| !v.passed)
            .map(|v| v.reason.clone())
    }

    /// Whether the gate passed thread `id` on commit `head`; `None` under
    /// GitHub's policy, where the approval tells instead.
    pub fn passed(&self, id: &str, head: &str) -> Option<bool> {
        (self.policy == pr::Policy::Gate).then(|| self.verdict(id, head).is_some_and(|v| v.passed))
    }
}

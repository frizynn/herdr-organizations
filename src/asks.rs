//! The user's asks: the `## Asked` lines of a coordinator's status file
//! (skill/COORDINATOR.md), with the state the coordinator wrote and the one
//! its links confirm. A link is confirmed from records that already exist (a
//! thread's state, the pull request state the ticker cached on it, a task
//! being done), so reading asks never runs gh, herdr or a tracker. The text
//! of an ask is the user's and the coordinator's words: it is shown, and it
//! never goes into a prompt.

use serde::{Deserialize, Serialize};

use crate::tasks::Task;
use crate::thread::Thread;

/// What the coordinator wrote in brackets; `unknown` for a line that does
/// not follow the format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum State {
    Open,
    Working,
    NeedsDecision,
    Answered,
    Done,
    #[default]
    Unknown,
}

impl State {
    pub fn word(self) -> &'static str {
        match self {
            State::Open => "open",
            State::Working => "working",
            State::NeedsDecision => "needs-decision",
            State::Answered => "answered",
            State::Done => "done",
            State::Unknown => "unknown",
        }
    }
}

/// What a link names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LinkKind {
    Thread,
    Pr,
    Task,
    #[default]
    Page,
}

/// How far the work an ask links to got, as its records say.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Confirmed {
    /// No link has a state the records know.
    #[default]
    #[serde(rename = "")]
    Unconfirmed,
    /// A linked thread works or waits on the user, a linked pull request is
    /// open, or a linked task is still to do.
    InProgress,
    /// A linked thread stopped without being resolved (idle, ready for
    /// review, landing), and nothing linked is in progress.
    Stopped,
    /// Nothing linked is in progress or stopped, and a linked thread failed
    /// or a linked pull request was closed without merging.
    Failed,
    /// Every linked thread is resolved, pull request merged and task done.
    Done,
}

/// One link of an ask, and what the records say about it.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Link {
    pub kind: LinkKind,
    /// As written in the status file.
    pub text: String,
    /// The thread id, pull request URL, task id or page URL it names;
    /// empty when the records do not know it.
    pub target: String,
    /// The thread it names, or whose pull request it is; empty otherwise.
    pub thread: String,
    /// That thread's title, or the task's.
    pub title: String,
    /// A thread's group token, `failed` or `missing`; a pull request's
    /// `open`, `merged`, `closed`, or empty before the ticker read it; a
    /// task's `todo`, `done` or `missing`; empty for a page.
    pub state: String,
}

/// One line under `## Asked`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Ask {
    /// Its line in the status file, from 1.
    pub line: usize,
    pub state: State,
    pub text: String,
    pub answer: String,
    pub links: Vec<Link>,
    pub confirmed: Confirmed,
    /// Why the state the coordinator wrote does not close with what its
    /// links confirm, in this binary's words; empty when it does.
    pub mismatch: String,
}

impl Ask {
    /// The order a list of asks is read in: what waits on the user, what
    /// does not close, what is in progress, what is done, then lines that
    /// do not follow the format.
    pub fn rank(&self) -> u8 {
        match self.state {
            State::NeedsDecision => 0,
            _ if !self.mismatch.is_empty() => 1,
            State::Open | State::Working => 2,
            State::Answered | State::Done => 3,
            State::Unknown => 4,
        }
    }
}

/// What a status file says, each ask confirmed against the records.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Contents {
    /// The `Objective:` line, empty without one.
    pub objective: String,
    pub asks: Vec<Ask>,
    /// The lines under `## Needs your decision`.
    pub needs_decision: Vec<String>,
}

/// What an ask is confirmed against: the project's thread records and the
/// tasks already on disk.
#[derive(Debug, Clone, Default)]
pub struct Records {
    threads: Vec<Thread>,
    tasks: Vec<Task>,
}

impl Records {
    pub fn new(threads: Vec<Thread>, tasks: Vec<Task>) -> Records {
        Records { threads, tasks }
    }
}

/// Reads a status file's text. It never fails: a line under `## Asked` that
/// does not follow the format is an ask in state `unknown` with its text.
pub fn read(_text: &str, _records: &Records) -> Contents {
    Contents::default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thread::{Group, Status};

    fn thread(id: &str, status: Status, group: Group, pr: &str, pr_state: &str) -> Thread {
        Thread {
            id: id.into(),
            title: format!("title {id}"),
            status,
            last_group: group.token().into(),
            pr: pr.into(),
            pr_state: pr_state.into(),
            ..Thread::default()
        }
    }

    fn records() -> Records {
        let pr = |n: u32| format!("https://github.com/acme/app/pull/{n}");
        Records::new(
            vec![
                thread("t-0001", Status::Open, Group::Working, "", ""),
                thread("t-0002", Status::Open, Group::Idle, "", ""),
                thread("t-0003", Status::Resolved, Group::Resolved, &pr(12), "MERGED"),
                thread("t-0004", Status::Open, Group::ReadyForReview, &pr(13), "OPEN"),
                thread("t-0005", Status::Failed, Group::Idle, "", ""),
                thread("t-0006", Status::Resolved, Group::Resolved, &pr(14), "CLOSED"),
                thread("t-0007", Status::Open, Group::WaitingOnYou, "", ""),
            ],
            crate::tasks::parse("## Now\n- [ ] Write the docs\n- [x] Ship it\n"),
        )
    }

    fn one(line: &str) -> Ask {
        let asks = read(&format!("# Status\n## Asked\n{line}\n"), &records()).asks;
        assert_eq!(asks.len(), 1, "{line}: {asks:?}");
        asks.into_iter().next().unwrap()
    }

    #[test]
    fn the_empty_template_a_file_without_asked_and_odd_lines_read_without_error() {
        let template = read(crate::status_file::TEMPLATE, &records());
        assert_eq!(template.objective, "(none)");
        assert!(template.asks.is_empty() && template.needs_decision.is_empty());

        let plain = read("Just notes, no sections.\n", &records());
        assert_eq!(plain, Contents::default());

        let odd = read(
            "# Status\n## Asked\n- (none)\nsome prose line\n- [blocked] Deploy : later\n- [done]\n\n## Needs your decision\n- none\n- Pilot : yes or no (https://example.com/plan)\n",
            &records(),
        );
        let states: Vec<(State, &str)> = odd
            .asks
            .iter()
            .map(|a| (a.state, a.text.as_str()))
            .collect();
        assert_eq!(
            states,
            [
                (State::Unknown, "some prose line"),
                (State::Unknown, "[blocked] Deploy : later"),
                (State::Done, ""),
            ]
        );
        assert_eq!(odd.asks[0].line, 4);
        assert_eq!(
            odd.needs_decision,
            ["Pilot : yes or no (https://example.com/plan)"]
        );
    }

    #[test]
    fn an_ask_has_its_text_answer_and_the_links_of_its_final_parenthesis() {
        let ask = one(
            "- [working] Fix login: the redirect : PR open (t-0004, #13 https://example.com/plan task:2)",
        );
        assert_eq!(ask.state, State::Working);
        assert_eq!(ask.text, "Fix login: the redirect");
        assert_eq!(ask.answer, "PR open");
        let links: Vec<(LinkKind, &str, &str, &str)> = ask
            .links
            .iter()
            .map(|l| (l.kind, l.target.as_str(), l.thread.as_str(), l.state.as_str()))
            .collect();
        assert_eq!(
            links,
            [
                (LinkKind::Thread, "t-0004", "t-0004", "ready-for-review"),
                (
                    LinkKind::Pr,
                    "https://github.com/acme/app/pull/13",
                    "t-0004",
                    "open"
                ),
                (LinkKind::Page, "https://example.com/plan", "", ""),
                (LinkKind::Task, "2", "", "todo"),
            ]
        );
        assert_eq!(ask.links[3].title, "Write the docs");
        assert_eq!(ask.confirmed, Confirmed::InProgress);

        // A parenthesis with no link in it stays part of the answer.
        let prose = one("- [answered] Is it fast? : yes (measured twice)");
        assert_eq!(prose.answer, "yes (measured twice)");
        assert!(prose.links.is_empty());

        // A full pull request URL and one the records do not know.
        let url = one(
            "- [done] Ship : merged (https://github.com/Acme/App/pull/12/files, https://github.com/acme/other/pull/9, t-9999, #77)",
        );
        let states: Vec<(&str, &str)> = url
            .links
            .iter()
            .map(|l| (l.target.as_str(), l.state.as_str()))
            .collect();
        assert_eq!(
            states,
            [
                ("https://github.com/acme/app/pull/12", "merged"),
                ("https://github.com/acme/other/pull/9", ""),
                ("", "missing"),
                ("", ""),
            ]
        );
        assert_eq!(url.links[0].thread, "t-0003");
    }

    /// The mismatch table, one row per case, then the cases that close.
    #[test]
    fn an_ask_does_not_close_when_its_state_and_its_links_disagree() {
        let cases = [
            ("done", "(#13)", "a linked PR is still open"),
            ("answered", "(t-0004 #13)", "a linked PR is still open"),
            ("done", "(t-0001)", "a linked thread is still working"),
            ("answered", "(t-0007)", "a linked thread is still working"),
            ("open", "(t-0003)", "a linked thread is resolved"),
            ("working", "(t-0005)", "a linked thread failed"),
            ("working", "(#12)", "a linked PR is merged"),
            ("open", "(#14)", "a linked PR is closed"),
            ("working", "(task:3)", "a linked task is done"),
            ("working", "", "no thread, PR or task is linked"),
        ];
        for (state, links, reason) in cases {
            let ask = one(&format!("- [{state}] Something : next {links}"));
            assert_eq!(ask.mismatch, reason, "[{state}] {links}");
        }
        let closes = [
            ("done", "(t-0003, #12)"),
            ("done", "(t-0002)"),
            ("answered", "(task:2)"),
            ("done", ""),
            ("open", ""),
            ("answered", "(https://example.com/x)"),
            ("working", "(https://example.com/x)"),
            ("working", "(t-0001)"),
            ("working", "(t-0003, t-0002)"),
            ("open", "(t-0003, #13)"),
            ("working", "(t-9999)"),
            ("needs-decision", "(t-0003)"),
            ("needs-decision", ""),
        ];
        for (state, links) in closes {
            let ask = one(&format!("- [{state}] Something : next {links}"));
            assert_eq!(ask.mismatch, "", "[{state}] {links}");
        }
        assert_eq!(one("- [blocked] x : y").mismatch, "");
    }

    #[test]
    fn the_confirmed_state_is_the_least_finished_link() {
        let confirmed = |links: &str| one(&format!("- [open] x : y ({links})")).confirmed;
        assert_eq!(confirmed("t-0003 t-0001"), Confirmed::InProgress);
        assert_eq!(confirmed("t-0003 t-0002"), Confirmed::Stopped);
        assert_eq!(confirmed("t-0003 t-0005"), Confirmed::Failed);
        assert_eq!(confirmed("t-0003 #12 task:3"), Confirmed::Done);
        assert_eq!(confirmed("https://example.com t-9999"), Confirmed::Unconfirmed);
        assert_eq!(one("- [open] x : y").confirmed, Confirmed::Unconfirmed);
    }

    #[test]
    fn asks_rank_decisions_then_mismatches_then_work_then_done() {
        let file = "## Asked\n- [done] a : b\n- [working] c : d\n- [open] e : f (t-0003)\n- [needs-decision] g : h\n- odd\n- [answered] i : j\n";
        let mut asks = read(file, &records()).asks;
        asks.sort_by_key(Ask::rank);
        let texts: Vec<&str> = asks.iter().map(|a| a.text.as_str()).collect();
        assert_eq!(texts, ["g", "c", "e", "a", "i", "odd"]);
    }
}

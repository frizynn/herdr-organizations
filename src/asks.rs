//! The user's asks: the `## Asked` lines of a coordinator's status file
//! (skill/COORDINATOR.md), with the state the coordinator wrote and the one
//! its links confirm. A link is confirmed from records that already exist (a
//! thread's state, the pull request state the ticker cached on it, a task
//! being done), so reading asks never runs gh, herdr or a tracker. The text
//! of an ask is the user's and the coordinator's words: it is shown, and it
//! never goes into a prompt.

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::organizations::{self, ROOT_ID};
use crate::paths::Ctx;
use crate::project::{self, Project};
use crate::status_file::{self, StatusJson};
use crate::tasks::{self, Task};
use crate::thread::{self, Group, Thread};
use crate::{pr, sidebar, threads};

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

/// How far the work an ask links to got, as its records say. In the order
/// a summary prefers: the least finished link wins.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Confirmed {
    /// No link has a state the records know.
    #[default]
    #[serde(rename = "")]
    Unconfirmed,
    /// Every linked thread is resolved, pull request merged and task done.
    Done,
    /// Nothing linked is in progress or stopped, and a linked thread failed
    /// or a linked pull request was closed without merging.
    Failed,
    /// A linked thread stopped without being resolved (idle, ready for
    /// review, landing), and nothing linked is in progress.
    Stopped,
    /// A linked thread works or waits on the user, a linked pull request is
    /// open, or a linked task is still to do.
    InProgress,
}

impl Confirmed {
    pub fn word(self) -> &'static str {
        match self {
            Confirmed::Unconfirmed => "",
            Confirmed::Done => "done",
            Confirmed::Failed => "failed",
            Confirmed::Stopped => "stopped",
            Confirmed::InProgress => "in-progress",
        }
    }
}

/// One link of an ask, and what the records say about it.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Link {
    pub kind: LinkKind,
    /// As written in the status file.
    pub text: String,
    /// The thread id, pull request URL, task id or page URL it names;
    /// empty for a thread id, `#N` or task the records do not know.
    pub target: String,
    /// The thread it names, or whose pull request it is; empty otherwise.
    pub thread: String,
    /// That thread's title, or the task's.
    pub title: String,
    /// A thread's group token, `failed` or `missing`; a pull request's
    /// `open`, `merged`, `closed`, or empty before the ticker read it; a
    /// task's `todo`, `done` or `missing`; empty for a page.
    pub state: String,
    /// The pull request behind a thread or pull request link, and its state
    /// as `state` words it; empty when there is none.
    pub pr: String,
    pub pr_state: String,
}

impl Link {
    /// `t-0007 working (PR open)`, `#12 merged`, `task:3 done`, a page's URL.
    pub fn words(&self) -> String {
        let mut words = self.text.clone();
        if !self.state.is_empty() {
            words = format!("{words} {}", self.state);
        }
        if self.kind == LinkKind::Thread && !self.pr_state.is_empty() {
            words = format!("{words} (PR {})", self.pr_state);
        }
        words
    }
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
    /// `<ask> : <answer>`, or the ask alone.
    pub fn said(&self) -> String {
        if self.answer.is_empty() {
            self.text.clone()
        } else {
            format!("{} : {}", self.text, self.answer)
        }
    }

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

/// What a resolved coordinator's file still shows, in the list and on its
/// card. It can no longer update the file or take an answer, so nothing in
/// it waits on the user (its decisions and `needs-decision` asks), and what
/// it marked answered or done and that closes is history. What it left open
/// and what does not close still show.
pub fn listed(ask: &Ask, coordinator_resolved: bool) -> bool {
    let history = match ask.state {
        State::NeedsDecision => true,
        State::Answered | State::Done => ask.mismatch.is_empty(),
        _ => false,
    };
    !(coordinator_resolved && history)
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

    pub fn load(ctx: &Ctx, project: &Project) -> Records {
        Records::new(thread::list(project), tasks::on_disk(ctx, project))
    }

    fn thread(&self, id: &str) -> Option<&Thread> {
        self.threads.iter().find(|t| t.id == id)
    }

    /// The thread whose pull request this is; with several, the newest.
    /// `None` when the matching threads have different pull requests, as
    /// `#12` of two repositories does.
    fn pr_thread(&self, is_it: impl Fn(&str) -> bool) -> Option<&Thread> {
        let mut matching = self
            .threads
            .iter()
            .rev()
            .filter(|t| !t.pr.is_empty() && is_it(&t.pr));
        let newest = matching.next()?;
        matching
            .all(|t| t.pr.eq_ignore_ascii_case(&newest.pr))
            .then_some(newest)
    }
}

/// A coordinator's status file, with what it says.
pub struct Sheet {
    /// `root` or the coordinator node's id.
    pub coordinator: String,
    /// The node's title, empty for the root.
    pub title: String,
    pub resolved: bool,
    pub status: StatusJson,
}

/// The status files of a project's coordinators that exist: the root's,
/// then each spawning coordinator node's in tree order. Read the way
/// `--json` reads them (`status_file::read`), through the path functions.
pub fn sheets(project: &Project, records: &Records) -> Vec<Sheet> {
    let root = status_file::read(&status_file::root(project)).map(|status| Sheet {
        coordinator: ROOT_ID.into(),
        title: String::new(),
        resolved: false,
        status,
    });
    let nodes = organizations::tree(project)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|entry| {
            let t = entry.thread;
            let status = organizations::node_status_json(project, &t)?;
            Some(Sheet {
                coordinator: t.id.clone(),
                title: clean(&t.title),
                resolved: t.status == thread::Status::Resolved,
                status,
            })
        });
    root.into_iter()
        .chain(nodes)
        .map(|sheet| Sheet {
            status: sheet.status.confirmed(records),
            ..sheet
        })
        .collect()
}

/// One row of `asks --json` (docs/json.md).
#[derive(Serialize)]
struct Row {
    project: String,
    coordinator: CoordinatorRef,
    #[serde(flatten)]
    ask: Ask,
}

#[derive(Serialize)]
struct CoordinatorRef {
    id: String,
    title: String,
}

/// `asks [--project P] [--json]`: every listed ask of every project that is
/// not archived, or of one, most pressing first (`Ask::rank`), else in
/// project, coordinator and line order. It only reads.
pub fn print(ctx: &Ctx, slug: Option<&str>, json: bool) -> Result<()> {
    let slugs = match slug {
        Some(slug) => {
            project::validate_slug(slug)?;
            vec![slug.to_string()]
        }
        None => crate::overview::visible_slugs(ctx),
    };
    let mut rows = Vec::new();
    for slug in slugs {
        let project = Project::load(&ctx.root, &slug)?;
        let records = Records::load(ctx, &project);
        for sheet in sheets(&project, &records) {
            for ask in sheet.status.asks {
                if listed(&ask, sheet.resolved) {
                    rows.push(Row {
                        project: project.slug.clone(),
                        coordinator: CoordinatorRef {
                            id: sheet.coordinator.clone(),
                            title: sheet.title.clone(),
                        },
                        ask,
                    });
                }
            }
        }
    }
    rows.sort_by_key(|row| row.ask.rank());
    if json {
        return threads::print_json(&serde_json::json!({
            "schema_version": threads::JSON_SCHEMA_VERSION,
            "asks": rows,
        }));
    }
    for row in &rows {
        let coordinator = if row.coordinator.title.is_empty() {
            row.coordinator.id.clone()
        } else {
            format!("{} {}", row.coordinator.id, row.coordinator.title)
        };
        let ask = &row.ask;
        let said = ask.said();
        let links: Vec<String> = ask.links.iter().map(Link::words).collect();
        println!(
            "{}\t{coordinator}\t{}\t{}\t{}\t{said}\t{}",
            row.project,
            ask.state.word(),
            ask.confirmed.word(),
            ask.mismatch,
            links.join(", ")
        );
    }
    Ok(())
}

/// Reads a status file's text. It never fails: a line under `## Asked` that
/// does not follow the format is an ask in state `unknown` with its text.
pub fn read(text: &str, records: &Records) -> Contents {
    let mut contents = Contents::default();
    let mut section = Section::Other;
    for (index, line) in text.lines().enumerate() {
        let line = line.trim();
        if contents.objective.is_empty()
            && let Some(objective) = line.strip_prefix("Objective:")
        {
            if !is_placeholder(objective) {
                contents.objective = clean(objective);
            }
            continue;
        }
        if line.starts_with('#') {
            section = Section::of(line);
            continue;
        }
        if line.is_empty() || is_placeholder(line) {
            continue;
        }
        match section {
            Section::Asked => contents.asks.push(ask(index + 1, line, records)),
            Section::NeedsDecision => contents
                .needs_decision
                .push(clean(bullet(line).unwrap_or(line))),
            Section::Other => {}
        }
    }
    contents
}

#[derive(Clone, Copy)]
enum Section {
    Asked,
    NeedsDecision,
    Other,
}

impl Section {
    fn of(heading: &str) -> Section {
        let name = heading.strip_prefix("## ").unwrap_or("").trim();
        if name.eq_ignore_ascii_case("asked") {
            Section::Asked
        } else if name.eq_ignore_ascii_case("needs your decision") {
            Section::NeedsDecision
        } else {
            Section::Other
        }
    }
}

/// The text after a `- ` or `* ` bullet.
fn bullet(line: &str) -> Option<&str> {
    line.strip_prefix("- ")
        .or_else(|| line.strip_prefix("* "))
        .map(str::trim)
}

/// `- (none)` and the like, which coordinators write under an empty heading.
fn is_placeholder(line: &str) -> bool {
    let text = bullet(line).unwrap_or(line).trim();
    let text = text.strip_prefix('(').unwrap_or(text);
    let text = text.trim_end_matches('.');
    let text = text.strip_suffix(')').unwrap_or(text);
    text.trim().eq_ignore_ascii_case("none")
}

/// One line, without control characters: the text is shown in a terminal.
fn clean(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .trim()
        .to_string()
}

/// `- [state] <ask> : <answer> (<links>)`; anything else is `unknown`.
fn ask(line: usize, text: &str, records: &Records) -> Ask {
    let unknown = |text: &str| Ask {
        line,
        text: clean(text),
        ..Ask::default()
    };
    let Some(body) = bullet(text) else {
        return unknown(text);
    };
    let Some((word, rest)) = body.strip_prefix('[').and_then(|b| b.split_once(']')) else {
        return unknown(body);
    };
    let Some(state) = [
        State::Open,
        State::Working,
        State::NeedsDecision,
        State::Answered,
        State::Done,
    ]
    .into_iter()
    .find(|s| s.word().eq_ignore_ascii_case(word.trim())) else {
        return unknown(body);
    };
    let (rest, tokens) = split_links(rest.trim());
    let (said, answer) = rest.split_once(" : ").unwrap_or((rest, ""));
    let links: Vec<Link> = tokens.iter().map(|t| link(t, records)).collect();
    Ask {
        line,
        state,
        text: clean(said),
        answer: clean(answer),
        confirmed: confirmed(&links),
        mismatch: mismatch(state, &links).unwrap_or_default().to_string(),
        links,
    }
}

/// The text before a final parenthesis and the links in it, separated by
/// commas or spaces. A parenthesis with no link in it is part of the text.
fn split_links(text: &str) -> (&str, Vec<&str>) {
    let Some(inner) = text.strip_suffix(')') else {
        return (text, Vec::new());
    };
    let mut depth = 1;
    let Some(open) = inner.char_indices().rev().find_map(|(i, c)| {
        match c {
            ')' => depth += 1,
            '(' => depth -= 1,
            _ => {}
        }
        (depth == 0).then_some(i)
    }) else {
        return (text, Vec::new());
    };
    let tokens: Vec<&str> = inner[open + 1..]
        .split(|c: char| c == ',' || c.is_whitespace())
        .map(|t| t.trim_end_matches(['.', ';', ':']))
        .filter(|t| kind(t).is_some())
        .collect();
    if tokens.is_empty() {
        return (text, Vec::new());
    }
    (text[..open].trim_end(), tokens)
}

/// What a token in the links parenthesis names, `None` for other words.
fn kind(token: &str) -> Option<LinkKind> {
    if token.strip_prefix("task:").is_some_and(|id| !id.is_empty()) {
        Some(LinkKind::Task)
    } else if thread::validate_id(token).is_ok() {
        Some(LinkKind::Thread)
    } else if pr_number(token).is_some() || pr_url(token).is_some() {
        Some(LinkKind::Pr)
    } else if token.starts_with("https://") || token.starts_with("http://") {
        Some(LinkKind::Page)
    } else {
        None
    }
}

/// `#12`: a pull request of one of the project's threads.
fn pr_number(token: &str) -> Option<&str> {
    token
        .strip_prefix('#')
        .filter(|n| (1..=9).contains(&n.len()) && n.chars().all(|c| c.is_ascii_digit()))
}

/// A pull request URL, without what follows its number (`/files`).
fn pr_url(token: &str) -> Option<String> {
    let rest = token.strip_prefix("https://github.com/")?;
    let url = format!(
        "https://github.com/{}",
        rest.split('/').take(4).collect::<Vec<_>>().join("/")
    );
    pr::valid_pr_url(&url).then_some(url)
}

fn link(token: &str, records: &Records) -> Link {
    let kind = kind(token).unwrap_or_default();
    let mut link = Link {
        kind,
        text: clean(token),
        ..Link::default()
    };
    let of_thread = |link: &mut Link, t: &Thread| {
        link.thread = t.id.clone();
        link.title = clean(&t.title);
        link.pr = t.pr.clone();
        link.pr_state = pr_state(t);
    };
    match kind {
        LinkKind::Thread => match records.thread(token) {
            Some(t) => {
                link.target = t.id.clone();
                link.state = thread_state(t).into();
                of_thread(&mut link, t);
            }
            None => link.state = "missing".into(),
        },
        LinkKind::Pr => {
            let url = pr_url(token);
            let found = match (pr_number(token), &url) {
                (Some(number), _) => records.pr_thread(|pr| sidebar::pr_number(pr) == Some(number)),
                (None, Some(url)) => {
                    let (repo, number) = pr::url_parts(url).unwrap_or_default();
                    records.pr_thread(|pr| {
                        pr::url_parts(pr)
                            .is_some_and(|(r, n)| r.eq_ignore_ascii_case(&repo) && n == number)
                    })
                }
                (None, None) => None,
            };
            match found {
                Some(t) => {
                    link.target = t.pr.clone();
                    link.state = pr_state(t);
                    of_thread(&mut link, t);
                }
                None => link.target = url.unwrap_or_default(),
            }
        }
        LinkKind::Task => {
            let id = token.strip_prefix("task:").unwrap_or_default();
            match tasks::linked(&records.tasks, id) {
                Some(task) => {
                    link.target = id.to_string();
                    link.title = clean(&task.title);
                    link.state = if task.done { "done" } else { "todo" }.into();
                }
                None => link.state = "missing".into(),
            }
        }
        LinkKind::Page => link.target = token.to_string(),
    }
    link
}

/// A thread's pull request state as the ticker last cached it on the
/// record: `open`, `merged`, `closed`, or empty.
fn pr_state(t: &Thread) -> String {
    if t.pr.is_empty() {
        String::new()
    } else if t.pr_merged() {
        "merged".into()
    } else {
        t.pr_state.to_ascii_lowercase()
    }
}

/// A thread's group as the ticker last recorded it, or `failed`.
fn thread_state(t: &Thread) -> &'static str {
    match t.status {
        thread::Status::Failed => "failed",
        thread::Status::Starting => Group::Working.token(),
        _ => threads::recorded_group(t).token(),
    }
}

/// How far a link got; a thread's own pull request counts with it.
fn progress(link: &Link) -> Confirmed {
    let own = own_progress(link);
    match (link.kind, link.pr_state.as_str()) {
        (LinkKind::Thread, "open") => Confirmed::InProgress,
        (LinkKind::Thread, "closed") => own.max(Confirmed::Failed),
        _ => own,
    }
}

fn own_progress(link: &Link) -> Confirmed {
    match (link.kind, link.state.as_str()) {
        (LinkKind::Thread, "working" | "waiting-on-you")
        | (LinkKind::Pr, "open")
        | (LinkKind::Task, "todo") => Confirmed::InProgress,
        (LinkKind::Thread, "ready-for-review" | "landing" | "idle") => Confirmed::Stopped,
        (LinkKind::Thread, "failed") | (LinkKind::Pr, "closed") => Confirmed::Failed,
        (LinkKind::Thread, "resolved") | (LinkKind::Pr, "merged") | (LinkKind::Task, "done") => {
            Confirmed::Done
        }
        _ => Confirmed::Unconfirmed,
    }
}

/// The least finished of the links.
fn confirmed(links: &[Link]) -> Confirmed {
    links.iter().map(progress).max().unwrap_or_default()
}

/// Why an ask in `state` does not close with its links, one row per case,
/// the first that matches wins, with the reason of its first link in that
/// state. `None` when it closes, and when nothing confirms it: an ask with
/// no link is a mismatch only while it is working. An answer can be that
/// the work failed, so only `done` does not close on a failed link.
fn mismatch(state: State, links: &[Link]) -> Option<&'static str> {
    let first = |wanted: &[Confirmed]| {
        links
            .iter()
            .find(|l| wanted.contains(&progress(l)))
            .and_then(reason)
    };
    match (state, confirmed(links)) {
        (State::Done | State::Answered, Confirmed::InProgress) => first(&[Confirmed::InProgress]),
        (State::Done, Confirmed::Failed) => first(&[Confirmed::Failed]),
        (State::Working, _) if links.is_empty() => Some("no thread, PR or task is linked"),
        (State::Open | State::Working, Confirmed::Done | Confirmed::Failed) => {
            first(&[Confirmed::Done, Confirmed::Failed])
        }
        _ => None,
    }
}

/// What puts a link where `progress` does, in the words of `mismatch`.
fn reason(link: &Link) -> Option<&'static str> {
    Some(
        match (link.kind, link.state.as_str(), link.pr_state.as_str()) {
            (LinkKind::Thread, "working" | "waiting-on-you", _) => {
                "a linked thread is still working"
            }
            (LinkKind::Thread | LinkKind::Pr, _, "open") => "a linked PR is still open",
            (LinkKind::Task, "todo", _) => "a linked task is still to do",
            (LinkKind::Thread, "failed", _) => "a linked thread failed",
            (LinkKind::Thread | LinkKind::Pr, _, "closed") => "a linked PR is closed",
            (LinkKind::Thread, "resolved", _) => "a linked thread is resolved",
            (LinkKind::Pr, _, "merged") => "a linked PR is merged",
            (LinkKind::Task, "done", _) => "a linked task is done",
            _ => return None,
        },
    )
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
                thread(
                    "t-0003",
                    Status::Resolved,
                    Group::Resolved,
                    &pr(12),
                    "MERGED",
                ),
                thread(
                    "t-0004",
                    Status::Open,
                    Group::ReadyForReview,
                    &pr(13),
                    "OPEN",
                ),
                thread("t-0005", Status::Failed, Group::Idle, "", ""),
                thread(
                    "t-0006",
                    Status::Resolved,
                    Group::Resolved,
                    &pr(14),
                    "CLOSED",
                ),
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
        assert_eq!(template.objective, "", "the template's (none) is unset");
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
            .map(|l| {
                (
                    l.kind,
                    l.target.as_str(),
                    l.thread.as_str(),
                    l.state.as_str(),
                )
            })
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
        // A thread brings its pull request along.
        assert_eq!(
            (ask.links[0].pr.as_str(), ask.links[0].pr_state.as_str()),
            ("https://github.com/acme/app/pull/13", "open")
        );
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
            ("done", "(t-0004)", "a linked PR is still open"),
            ("answered", "(t-0004 #13)", "a linked PR is still open"),
            ("done", "(t-0001)", "a linked thread is still working"),
            ("answered", "(t-0007)", "a linked thread is still working"),
            ("open", "(t-0003)", "a linked thread is resolved"),
            ("working", "(t-0005)", "a linked thread failed"),
            ("working", "(#12)", "a linked PR is merged"),
            ("open", "(#14)", "a linked PR is closed"),
            ("working", "(task:3)", "a linked task is done"),
            ("working", "", "no thread, PR or task is linked"),
            ("done", "(task:2)", "a linked task is still to do"),
            ("answered", "(task:2)", "a linked task is still to do"),
            ("done", "(t-0003, task:2)", "a linked task is still to do"),
            ("done", "(t-0005)", "a linked thread failed"),
            ("done", "(#14)", "a linked PR is closed"),
            ("done", "(t-0006)", "a linked PR is closed"),
        ];
        for (state, links, reason) in cases {
            let ask = one(&format!("- [{state}] Something : next {links}"));
            assert_eq!(ask.mismatch, reason, "[{state}] {links}");
        }
        let closes = [
            ("done", "(t-0003, #12)"),
            ("done", "(t-0002)"),
            ("done", "(task:3)"),
            ("answered", "(t-0005)"),
            ("answered", "(#14)"),
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

    /// `#12` names a pull request only when every thread with that number
    /// has the same one; with two repositories it is left unresolved.
    #[test]
    fn a_pr_number_two_repositories_share_resolves_to_nothing() {
        let records = Records::new(
            vec![
                thread(
                    "t-0001",
                    Status::Open,
                    Group::ReadyForReview,
                    "https://github.com/acme/app/pull/12",
                    "OPEN",
                ),
                thread(
                    "t-0002",
                    Status::Resolved,
                    Group::Resolved,
                    "https://github.com/acme/web/pull/12",
                    "MERGED",
                ),
                thread(
                    "t-0003",
                    Status::Open,
                    Group::Idle,
                    "https://github.com/acme/app/pull/7",
                    "OPEN",
                ),
                thread(
                    "t-0004",
                    Status::Resolved,
                    Group::Resolved,
                    "https://github.com/acme/app/pull/7",
                    "OPEN",
                ),
            ],
            Vec::new(),
        );
        let ask = |links: &str| {
            read(
                &format!("## Asked\n- [done] Ship : merged ({links})\n"),
                &records,
            )
            .asks[0]
                .clone()
        };
        let ambiguous = ask("#12");
        assert_eq!(
            (
                ambiguous.links[0].target.as_str(),
                ambiguous.links[0].state.as_str()
            ),
            ("", "")
        );
        assert_eq!(ambiguous.mismatch, "");
        let full = ask("https://github.com/acme/app/pull/12");
        assert_eq!(full.mismatch, "a linked PR is still open");
        let shared = ask("#7");
        assert_eq!(shared.links[0].target, "https://github.com/acme/app/pull/7");
        assert_eq!(shared.mismatch, "a linked PR is still open");
    }

    #[test]
    fn the_confirmed_state_is_the_least_finished_link() {
        let confirmed = |links: &str| one(&format!("- [open] x : y ({links})")).confirmed;
        assert_eq!(confirmed("t-0003 t-0001"), Confirmed::InProgress);
        assert_eq!(confirmed("t-0003 t-0002"), Confirmed::Stopped);
        assert_eq!(confirmed("t-0004"), Confirmed::InProgress, "its PR is open");
        assert_eq!(confirmed("t-0003 t-0005"), Confirmed::Failed);
        assert_eq!(confirmed("t-0003 #12 task:3"), Confirmed::Done);
        assert_eq!(
            confirmed("https://example.com t-9999"),
            Confirmed::Unconfirmed
        );
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

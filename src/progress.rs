//! Built-in progress self-reporting: the agent runs `report` in its own pane,
//! the harness hooks (`hook`) inject the instructions and a reminder, one JSON
//! record per pane under `<root>/.progress/` feeds the ticker. The binding is
//! the pane id Herdr hands every pane shell; there is nothing to mint and no
//! daemon. Outside a Herdr pane both commands do nothing.

use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

use crate::herdr::{CALL_TIMEOUT, Herdr};
use crate::paths::{Ctx, Env};
use crate::runner::Runner;

pub const ACTIVITY_COLUMNS: usize = 40;
pub const ACTIVITY_TTL_MS: u64 = 300_000;
/// A worker pane gets a PostToolUse check-in reminder at most this often;
/// coordinator panes get none.
pub const CHECK_IN_SECS: i64 = 300;
/// Prompts that start with one of these were typed by the ticker (a nudge,
/// a routine, a thread's brief line), the harness or another coordinator
/// (`organizations::signed`), not by the user: they get no reminder.
const AUTOMATED_PROMPTS: [&str; 7] = [
    "[hp inbox]",
    "[hp ticker]",
    "[hp routine ",
    "[herdr-projects ticker",
    "<task-notification>",
    "[from ",
    crate::thread::LAUNCH_PROMPT_PREFIX,
];
pub const WAITING: &str = "Waiting for you";
pub const DONE: &str = "Done";

/// One pane's self-report.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(default)]
pub struct Record {
    pub socket: String,
    pub pane_id: String,
    /// Pane ids restart from `w1` after a server restart; the terminal id tells
    /// a stale record from a live pane that reused the id.
    pub terminal_id: String,
    pub agent: String,
    pub activity: String,
    pub percent: Option<u8>,
    /// Unix seconds of the last `report`; 0 when none since the session started.
    pub reported_at: i64,
    /// Unix seconds of the last reminder the hook injected.
    pub reminded_at: i64,
    pub session_started_at: i64,
    /// The harness's own session id from the last SessionStart: Copilot CLI
    /// runs a subagent's hooks under another id with no other marker.
    pub session_id: String,
}

impl Record {
    pub fn waiting(&self) -> bool {
        self.reported_at > 0 && self.activity == WAITING
    }

    pub fn done(&self) -> bool {
        self.reported_at > 0 && self.percent == Some(100)
    }

    pub fn reported(&self) -> bool {
        self.reported_at > 0
    }
}

pub fn now() -> i64 {
    jiff::Timestamp::now().as_second()
}

/// A report recent enough for its activity to show in the sidebar: silence
/// clears it after `ACTIVITY_TTL_MS`.
pub fn fresh(record: &Record) -> bool {
    record.reported() && now() - record.reported_at < (ACTIVITY_TTL_MS / 1000) as i64
}

pub fn dir(root: &Path) -> PathBuf {
    root.join(".progress")
}

/// `<pane id>-<short hash of the socket path>.json`: pane ids repeat across sessions.
pub fn path(root: &Path, socket: &str, pane_id: &str) -> PathBuf {
    let hash = &crate::thread::sha256_hex(socket.as_bytes())[..8];
    dir(root).join(format!("{}-{hash}.json", pane_id.replace(':', "_")))
}

pub fn load(root: &Path, socket: &str, pane_id: &str) -> Option<Record> {
    crate::project::read_json(&path(root, socket, pane_id))
}

pub fn save(root: &Path, record: &Record) -> Result<()> {
    std::fs::create_dir_all(dir(root))?;
    crate::project::write_json(&path(root, &record.socket, &record.pane_id), record)
}

pub fn remove(root: &Path, socket: &str, pane_id: &str) {
    let _ = std::fs::remove_file(path(root, socket, pane_id));
}

/// Every record under the root, for the ticker's cleanup.
pub fn all(root: &Path) -> Vec<Record> {
    let Ok(entries) = std::fs::read_dir(dir(root)) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|e| crate::project::read_json::<Record>(&e.path()))
        .collect()
}

/// At most 40 columns, no control or bidi characters, trimmed. 100% is "Done".
pub fn clean(input: &str, columns: usize) -> String {
    let mut width = 0;
    input
        .chars()
        .filter(|c| {
            !c.is_control() && !matches!(*c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        })
        .take(120)
        .take_while(|c| {
            // East Asian wide characters take two columns; everything else one.
            width += if ('\u{1100}'..='\u{115f}').contains(c)
                || ('\u{2e80}'..='\u{a4cf}').contains(c)
                || ('\u{ac00}'..='\u{d7a3}').contains(c)
                || ('\u{f900}'..='\u{faff}').contains(c)
                || ('\u{fe30}'..='\u{fe4f}').contains(c)
                || ('\u{ff00}'..='\u{ff60}').contains(c)
                || ('\u{ffe0}'..='\u{ffe6}').contains(c)
                || ('\u{1f300}'..='\u{1f64f}').contains(c)
                || ('\u{1f900}'..='\u{1f9ff}').contains(c)
                || ('\u{20000}'..='\u{3fffd}').contains(c)
            {
                2
            } else {
                1
            };
            width <= columns
        })
        .collect::<String>()
        .trim()
        .to_string()
}

/// Who works in a pane the hooks speak in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneRole {
    /// The project's root coordinator or a coordinator node.
    Coordinator,
    Worker,
}

/// The calling pane, when this process runs inside a Herdr pane: the id
/// Herdr shows for it (a moved pane keeps its launch-time `HERDR_PANE_ID`),
/// its terminal id and the agent Herdr detects. `None` outside Herdr.
pub struct Current {
    pub socket: String,
    pub pane_id: String,
    pub terminal_id: String,
    pub agent: String,
    /// The whole `pane current` reply: workspace, tab and both directories.
    pub pane: crate::herdr::Pane,
}

pub fn current(env: &Env, runner: &dyn Runner) -> Option<Current> {
    current_within(env, runner, CALL_TIMEOUT)
}

/// The hook uses a short timeout: a slow server must not stall every tool call.
pub fn current_within(
    env: &Env,
    runner: &dyn Runner,
    timeout: std::time::Duration,
) -> Option<Current> {
    if env.var("HERDR_ENV") != Some("1") {
        return None;
    }
    env.var("HERDR_PANE_ID")?;
    let socket = env.var("HERDR_SOCKET_PATH")?.to_string();
    let herdr = Herdr::new(env.herdr_bin(), &socket, runner);
    let result = herdr
        .call(&["pane", "current", "--current"], timeout)
        .ok()?;
    let reply = &result["pane"];
    let pane_id = reply["pane_id"].as_str()?.to_string();
    let field = |name: &str| reply[name].as_str().unwrap_or("").to_string();
    let pane = crate::herdr::Pane {
        pane_id: pane_id.clone(),
        tab_id: field("tab_id"),
        workspace_id: field("workspace_id"),
        cwd: field("cwd"),
        foreground_cwd: field("foreground_cwd"),
        terminal_id: field("terminal_id"),
        ..Default::default()
    };
    Some(Current {
        socket,
        pane_id,
        terminal_id: pane.terminal_id.clone(),
        agent: field("agent"),
        pane,
    })
}

/// Whose pane the hooks speak in: a project's root coordinator, or an open
/// local thread on the same server (a worker or a coordinator node).
pub enum Seat {
    Root(crate::project::Project),
    Node(crate::project::Project, Box<crate::thread::Thread>),
}

impl Seat {
    pub fn project(&self) -> &crate::project::Project {
        match self {
            Seat::Root(project) | Seat::Node(project, _) => project,
        }
    }

    pub fn role(&self) -> PaneRole {
        match self {
            Seat::Root(_) => PaneRole::Coordinator,
            Seat::Node(_, t) if t.role == crate::thread::NodeRole::Coordinator => {
                PaneRole::Coordinator
            }
            Seat::Node(..) => PaneRole::Worker,
        }
    }
}

/// The calling pane's seat when it is one of a project's coordinators or an
/// open local thread on the same server: the only panes the hooks speak in.
/// A coordinator node is a thread record, so its pane counts as a thread's,
/// with the role of its record.
///
/// Every tool call in every pane on the machine runs the hook, so files are
/// read first and Herdr is asked only when the pane could be a project's (its
/// directory is a project home, or an open thread records its pane id). Fails
/// closed: when Herdr cannot list the agents, the hook stays silent.
fn managed(ctx: &Ctx, current: &Current) -> Option<Seat> {
    use crate::{coordinator, project, thread};
    let pane = &current.pane;
    // Ids are only unique within one server: a project whose coordinator was
    // recorded on another socket cannot own this pane. A project never
    // opened (no record) may still be coordinated by an agent started by hand.
    let projects: Vec<_> = project::list_slugs(&ctx.root)
        .iter()
        .filter_map(|slug| project::Project::load(&ctx.root, slug).ok())
        .map(|project| {
            let record = project.coordinator();
            (project, record)
        })
        .filter(|(_, record)| record.as_ref().is_none_or(|r| r.socket == current.socket))
        .collect();
    let in_home = |project: &project::Project| {
        let home = project.canonical_dir().to_string_lossy().into_owned();
        crate::herdr::own_dir(&pane.cwd, &pane.foreground_cwd) == home
    };
    // A thread record carries no socket of its own: only the project's
    // coordinator record ties its pane ids to this server.
    let threads = |project: &project::Project,
                   record: &Option<project::Coordinator>|
     -> Vec<thread::Thread> {
        if record.is_none() {
            return Vec::new();
        }
        thread::list(project)
            .into_iter()
            .filter(|t| {
                !t.is_remote()
                    && t.status != thread::Status::Resolved
                    && thread::pane_matches(t, pane)
            })
            .collect()
    };
    let candidates: Vec<_> = projects
        .iter()
        .filter(|(project, record)| in_home(project) || !threads(project, record).is_empty())
        .collect();
    if candidates.is_empty() {
        return None;
    }
    if let Some((project, _)) = candidates.iter().find(|(_, record)| {
        record
            .as_ref()
            .is_some_and(|r| coordinator::pane_matches(r, pane))
    }) {
        return Some(Seat::Root(project.clone()));
    }
    let herdr = Herdr::new(ctx.env.herdr_bin(), &current.socket, ctx.runner);
    let agents = herdr
        .call(&["agent", "list"], std::time::Duration::from_secs(2))
        .ok()
        .and_then(|reply| {
            serde_json::from_value::<Vec<crate::herdr::Agent>>(reply["agents"].clone()).ok()
        })?;
    let mut agents: Vec<_> = agents
        .into_iter()
        .filter(|a| {
            a.pane_id == pane.pane_id
                && a.workspace_id == pane.workspace_id
                && a.tab_id == pane.tab_id
                && a.terminal_id == pane.terminal_id
        })
        .collect();
    if agents.is_empty() && current.agent.is_empty() {
        // At SessionStart Herdr may not have detected the agent yet; the hook
        // itself shows one is starting here, so stand in the pane for it.
        agents.push(crate::herdr::Agent {
            pane_id: pane.pane_id.clone(),
            workspace_id: pane.workspace_id.clone(),
            tab_id: pane.tab_id.clone(),
            cwd: pane.cwd.clone(),
            foreground_cwd: pane.foreground_cwd.clone(),
            terminal_id: pane.terminal_id.clone(),
            ..Default::default()
        });
    }
    candidates.iter().find_map(|(project, record)| {
        // A coordinator `open --new` or the user started that the ticker has
        // not recorded yet: the ticker's own discovery rule.
        if coordinator::found(project, &current.socket, "", &agents)
            .is_some_and(|r| coordinator::pane_matches(&r, pane))
        {
            return Some(Seat::Root(project.clone()));
        }
        threads(project, record)
            .into_iter()
            .find(|t| agents.iter().any(|a| thread::agent_matches(t, a)))
            .map(|t| Seat::Node(project.clone(), Box::new(t)))
    })
}

/// A prompt the user typed to a coordinator that keeps a status file (the
/// root, or a coordinator node that may spawn): records its time, never its
/// text. A node whose STATUS.md is older than the user's previous message
/// gets one line saying so; the root's `context` lists it as drift instead.
pub fn note_human_prompt(seat: &Seat, event: &serde_json::Value, now: i64) -> Option<String> {
    if event["hook_event_name"] != "UserPromptSubmit"
        || automated(event["prompt"].as_str().unwrap_or(""))
    {
        return None;
    }
    use crate::status_file;
    match seat {
        Seat::Root(project) => {
            let _ = status_file::record_ask(project, status_file::ROOT_KEY, now);
            None
        }
        Seat::Node(project, t) if t.is_spawning_coordinator() => {
            let previous = status_file::record_ask(project, &t.id, now).ok()?;
            let path = crate::organizations::status_path(project, &t.id);
            status_file::stale(&path, previous).then(|| status_file::NODE_STALE.to_string())
        }
        Seat::Node(..) => None,
    }
}

/// The hook's whole answer in a project's pane: what [`respond`] gives, and
/// a coordinator node's stale status line after a prompt the user typed.
pub fn answer(
    seat: &Seat,
    settings: &crate::project::Settings,
    record: &mut Record,
    event: &serde_json::Value,
    prefix: &str,
    now: i64,
) -> Option<String> {
    let _ = settings;
    let parts: Vec<String> = [
        respond(record, event, seat.role(), prefix, now),
        note_human_prompt(seat, event, now),
    ]
    .into_iter()
    .flatten()
    .collect();
    (!parts.is_empty()).then(|| parts.join("\n\n"))
}

/// `report --percent N|--unknown --activity "..."`, run by the agent in its
/// pane. Writes the record; the ticker shows its activity on the pane's sub-line
/// while it is [`fresh`].
pub fn report(ctx: &Ctx, percent: Option<u8>, activity: &str) -> Result<()> {
    let Some(pane) = current(ctx.env, ctx.runner) else {
        println!("not inside a Herdr pane; nothing reported");
        return Ok(());
    };
    if percent.is_some_and(|p| p > 100) {
        bail!("--percent must be 0 to 100");
    }
    let activity = if percent == Some(100) {
        DONE.to_string()
    } else {
        clean(activity, ACTIVITY_COLUMNS)
    };
    if activity.is_empty() {
        bail!("--activity is empty");
    }
    let mut record = load(&ctx.root, &pane.socket, &pane.pane_id).unwrap_or_default();
    record.socket = pane.socket.clone();
    record.pane_id = pane.pane_id.clone();
    record.terminal_id = pane.terminal_id.clone();
    record.agent = pane.agent.clone();
    record.activity = activity.clone();
    record.percent = percent;
    record.reported_at = now();
    save(&ctx.root, &record)?;
    println!(
        "recorded: {}{activity}",
        percent.map(|p| format!("{p}% · ")).unwrap_or_default()
    );
    Ok(())
}

/// `progress [--pane ID]`: the record for the calling pane, or the given one.
pub fn print(ctx: &Ctx, pane: Option<&str>) -> Result<()> {
    let (socket, pane_id) = match pane {
        Some(id) => (
            ctx.env.var("HERDR_SOCKET_PATH").unwrap_or("").to_string(),
            id.to_string(),
        ),
        None => match current(ctx.env, ctx.runner) {
            Some(c) => (c.socket, c.pane_id),
            None => bail!("not inside a Herdr pane; pass --pane ID"),
        },
    };
    match load(&ctx.root, &socket, &pane_id) {
        Some(record) => println!("{}", serde_json::to_string_pretty(&record)?),
        None => println!("null"),
    }
    Ok(())
}

// ---------------------------------------------------------------- hooks

/// The instructions the SessionStart hook injects, with the exact command.
pub fn instructions(prefix: &str, pane_id: &str) -> String {
    format!(
        "# Progress (herdr-projects)\n\n{}",
        guidance(prefix, Some(pane_id))
    )
}

/// The body of the progress instructions. Hooks inject it at session start;
/// every other agent reads it in its thread brief or the coordinator skill,
/// so any harness that can run a shell command reports.
pub fn guidance(prefix: &str, pane_id: Option<&str>) -> String {
    let pane = pane_id
        .map(|id| format!("this pane ({id})"))
        .unwrap_or_else(|| "this pane".into());
    format!(
        "Report the progress of the user's whole current task through `{prefix} report`. This is your estimate, not a timer or a count of tools. Reporting failures must never stop the actual work: give one short diagnostic and continue, without retries.\n\n\
         Report a rough percentage in five-point increments and a two-to-four-word activity, such as `Reading code`, `Testing changes` or `Waiting for you`:\n\n\
         `{prefix} report --percent 25 --activity 'Reading code'`\n\
         `{prefix} report --unknown --activity 'Assessing task'`\n\n\
         Report at the start of new work, after meaningful milestones, when the activity changes, at blockers, and before every substantive reply. Report `--activity 'Waiting for you'` whenever you stop to ask the user something. During active work aim for one report per minute at a natural tool boundary; never invent progress to satisfy a reminder. Revise the estimate downward when you discover more work; use `--unknown` while the scope is unclear.\n\n\
         Use `--percent 100` only when the entire requested outcome and its checks are finished; it displays `Done`. If more work is requested afterwards, report a fresh, lower percentage.\n\n\
         Only the top-level agent in {pane} reports; subagents and helpers do not. Await each report command; do not run it in the background."
    )
}

pub fn reminder(prefix: &str) -> String {
    format!(
        "Progress check-in is due if this is a natural boundary: `{prefix} report --percent N --activity '...'`. Reassess the current task; do not invent progress. Report `Waiting for you` before a question to the user."
    )
}

/// Events the reporter reacts to: the top-level agent's own SessionStart,
/// UserPromptSubmit and PostToolUse, never a subagent's, and never the
/// PostToolUse of the `report` call itself.
pub fn eligible(event: &serde_json::Value) -> bool {
    if ["agent_id", "subagent_id", "agent_transcript_path"]
        .iter()
        .any(|key| event.get(key).is_some_and(|v| !v.is_null()))
    {
        return false;
    }
    if event["transcript_path"]
        .as_str()
        .is_some_and(|p| p.contains("/subagents/"))
    {
        return false;
    }
    let name = event["hook_event_name"].as_str().unwrap_or("");
    if !matches!(name, "SessionStart" | "PostToolUse" | "UserPromptSubmit") {
        return false;
    }
    if name == "PostToolUse"
        && crate::paths::names_our_binary(&event["tool_input"].to_string())
        && event["tool_input"].to_string().contains(" report ")
    {
        return false;
    }
    true
}

/// True for a prompt the user did not type (see `AUTOMATED_PROMPTS`).
pub fn automated(prompt: &str) -> bool {
    let prompt = prompt.trim_start();
    AUTOMATED_PROMPTS.iter().any(|p| prompt.starts_with(p))
}

/// What the hook answers for one event: the text to inject, and whether the
/// record changed. Pure, so it is testable without a pane. Coordinators get
/// no check-in reminder, and nobody gets one for an automated prompt.
pub fn respond(
    record: &mut Record,
    event: &serde_json::Value,
    role: PaneRole,
    prefix: &str,
    now: i64,
) -> Option<String> {
    match event["hook_event_name"].as_str().unwrap_or("") {
        "SessionStart" => {
            // A new session in this pane: the old report no longer describes it.
            let keep = (
                record.socket.clone(),
                record.pane_id.clone(),
                record.terminal_id.clone(),
                record.agent.clone(),
            );
            *record = Record {
                socket: keep.0,
                pane_id: keep.1,
                terminal_id: keep.2,
                agent: keep.3,
                session_started_at: now,
                reminded_at: now,
                ..Record::default()
            };
            Some(instructions(prefix, &record.pane_id))
        }
        "UserPromptSubmit" if automated(event["prompt"].as_str().unwrap_or("")) => None,
        "UserPromptSubmit" => {
            record.reminded_at = now;
            // The user answered: an old "Waiting for you" no longer holds.
            if record.activity == WAITING {
                record.activity.clear();
                record.reported_at = 0;
            }
            Some(format!(
                "Before task tools or a blocking question, check whether this request starts new work; if so report a fresh estimate. Then report before waiting for the user, for example `{prefix} report --unknown --activity 'Waiting for you'`."
            ))
        }
        "PostToolUse" if role == PaneRole::Coordinator => None,
        "PostToolUse" => {
            let done = record.percent == Some(100);
            let due = now - record.reminded_at >= CHECK_IN_SECS
                && !done
                && (record.reported_at == 0 || now - record.reported_at >= CHECK_IN_SECS);
            if due {
                record.reminded_at = now;
                Some(reminder(prefix))
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Whether an event belongs to another session than the one this pane
/// started: a subagent's, where the harness gives it an id of its own.
pub fn foreign_session(record: &Record, event: &serde_json::Value) -> bool {
    let id = event["session_id"].as_str().unwrap_or("");
    !record.session_id.is_empty() && !id.is_empty() && id != record.session_id
}

/// The event under its Claude Code name, which the rest of the reporter
/// uses: `hook_event_name` rewritten from the harness's own name.
pub fn normalize(
    harness: &crate::setup::Harness,
    mut event: serde_json::Value,
) -> serde_json::Value {
    let native = event["hook_event_name"].as_str().unwrap_or("");
    if let Some(i) = harness.events.iter().position(|e| *e == native) {
        event["hook_event_name"] = ["SessionStart", "UserPromptSubmit", "PostToolUse"][i].into();
    }
    event
}

/// The hook's answer in the shape the harness reads.
pub fn output(harness: &crate::setup::Harness, native: &str, text: &str) -> serde_json::Value {
    if harness.top_level_output {
        serde_json::json!({"additionalContext": text})
    } else {
        serde_json::json!({"hookSpecificOutput": {"hookEventName": native, "additionalContext": text}})
    }
}

/// `hook --agent <harness>`, the entry point the harness hooks run. Silent
/// (exit 0, no output, no record) outside a project's coordinator, coordinator
/// node and local thread panes, so the same hooks may sit in the user's
/// settings for every session on the machine.
pub fn hook(ctx: &Ctx, agent: &str) -> Result<()> {
    if ctx.env.var("HERDR_ENV") != Some("1") || ctx.env.var("HERDR_PANE_ID").is_none() {
        return Ok(());
    }
    use std::io::Read;
    let mut input = String::new();
    std::io::stdin()
        .take(1_048_576)
        .read_to_string(&mut input)?;
    let Ok(event) = serde_json::from_str::<serde_json::Value>(&input) else {
        return Ok(());
    };
    let Some(harness) = crate::setup::harness(agent) else {
        return Ok(());
    };
    let native = event["hook_event_name"].as_str().unwrap_or("").to_string();
    let event = normalize(harness, event);
    if !eligible(&event) {
        return Ok(());
    }
    let kind = event["hook_event_name"].as_str().unwrap_or("").to_string();
    // Codex runs hooks for nested threads too; only the pane's own thread reports.
    if agent == "codex"
        && let Some(native) = event["session_id"].as_str()
        && ctx
            .env
            .var("CODEX_THREAD_ID")
            .is_some_and(|id| id != native)
    {
        return Ok(());
    }
    // At SessionStart Herdr may not have detected the agent yet: wait briefly.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    let pane = loop {
        match current_within(ctx.env, ctx.runner, std::time::Duration::from_secs(2)) {
            Some(p) if kind != "SessionStart" || !p.agent.is_empty() => break p,
            Some(p) if std::time::Instant::now() >= deadline => break p,
            None if kind != "SessionStart" || std::time::Instant::now() >= deadline => {
                return Ok(());
            }
            _ => std::thread::sleep(std::time::Duration::from_millis(100)),
        }
    };
    if !pane.agent.is_empty() && pane.agent != agent {
        return Ok(()); // another harness's hook fired in a pane that is not its own
    }
    let Some(seat) = managed(ctx, &pane) else {
        return Ok(());
    };
    let mut record = load(&ctx.root, &pane.socket, &pane.pane_id).unwrap_or_default();
    if kind != "SessionStart" && foreign_session(&record, &event) {
        return Ok(());
    }
    record.socket = pane.socket.clone();
    record.pane_id = pane.pane_id.clone();
    record.terminal_id = pane.terminal_id.clone();
    record.agent = if pane.agent.is_empty() {
        agent.to_string()
    } else {
        pane.agent.clone()
    };
    let prefix = crate::coordinator::current_prefix(&ctx.root)?;
    let before = record.clone();
    let now = now();
    let settings = seat
        .project()
        .read_project_md()
        .map(|(settings, _)| settings)
        .unwrap_or_default();
    let text = answer(&seat, &settings, &mut record, &event, &prefix, now);
    if kind == "SessionStart" {
        record.session_id = event["session_id"].as_str().unwrap_or("").to_string();
    }
    if record != before {
        save(&ctx.root, &record)?;
    }
    if let Some(text) = text {
        println!("{}", output(harness, &native, &text));
    }
    Ok(())
}

/// The record's contribution to a thread's live state, or nothing when the
/// record is missing or describes an earlier pane with the same id.
pub fn self_report(root: &Path, socket: &str, pane_id: &str, terminal_id: &str) -> Option<Record> {
    let record = load(root, socket, pane_id)?;
    if !terminal_id.is_empty()
        && !record.terminal_id.is_empty()
        && record.terminal_id != terminal_id
    {
        return None;
    }
    record.reported().then_some(record)
}

/// Drops records whose pane is no longer listed in the session they belong to.
/// Only records of `socket` are judged: other sessions' records are theirs.
pub fn prune(root: &Path, socket: &str, live_pane_ids: &[String]) {
    for record in all(root) {
        if record.socket == socket && !live_pane_ids.contains(&record.pane_id) {
            remove(root, &record.socket, &record.pane_id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn an_open_thread_speaks_before_herdr_detects_its_agent() {
        use crate::runner::fake::{FakeRunner, ok};
        let home = tempfile::tempdir().unwrap();
        let project = crate::project::create(home.path(), "demo", "", vec![]).unwrap();
        project
            .update_coordinator(|c| c.socket = "/managed.sock".into())
            .unwrap();
        for role in [
            crate::thread::NodeRole::Worker,
            crate::thread::NodeRole::Coordinator,
        ] {
            let thread = crate::thread::Thread {
                id: "t-0001".into(),
                role,
                can_spawn: role == crate::thread::NodeRole::Coordinator,
                pane_id: "w1:p1".into(),
                cwd: "/worktree".into(),
                ..Default::default()
            };
            crate::project::write_atomic(
                &crate::thread::record_path(&project, &thread.id),
                toml::to_string(&thread).unwrap().as_bytes(),
            )
            .unwrap();
            let runner = FakeRunner::new();
            runner.on("agent list", ok(r#"{"result":{"agents":[]}}"#));
            let env = Env::for_test(home.path(), &[]);
            let ctx = Ctx {
                env: &env,
                root: home.path().into(),
                config_dir: home.path().join("config"),
                runner: &runner,
                detached_ticker: false,
            };
            let pane = crate::herdr::Pane {
                pane_id: "w1:p1".into(),
                tab_id: "w1:t1".into(),
                workspace_id: "w1".into(),
                cwd: "/worktree".into(),
                terminal_id: "term-1".into(),
                ..Default::default()
            };
            let current = |socket: &str| Current {
                socket: socket.into(),
                pane_id: "w1:p1".into(),
                terminal_id: "term-1".into(),
                agent: String::new(),
                pane: pane.clone(),
            };
            let expected = match role {
                crate::thread::NodeRole::Coordinator => PaneRole::Coordinator,
                crate::thread::NodeRole::Worker => PaneRole::Worker,
            };
            assert_eq!(
                managed(&ctx, &current("/managed.sock")).map(|s| s.role()),
                Some(expected)
            );
            // The same pane id on another server is not the thread's.
            assert!(managed(&ctx, &current("/other.sock")).is_none(), "{role:?}");
        }
    }

    #[test]
    fn the_root_coordinators_pane_is_a_coordinator_pane() {
        use crate::runner::fake::FakeRunner;
        let home = tempfile::tempdir().unwrap();
        let project = crate::project::create(home.path(), "demo", "", vec![]).unwrap();
        let cwd = project.canonical_dir().to_string_lossy().into_owned();
        project
            .update_coordinator(|c| {
                c.socket = "/managed.sock".into();
                c.workspace_id = "w1".into();
                c.tab_id = "w1:t1".into();
                c.pane_id = "w1:p1".into();
                c.cwd = cwd.clone();
            })
            .unwrap();
        let runner = FakeRunner::new();
        let env = Env::for_test(home.path(), &[]);
        let ctx = Ctx {
            env: &env,
            root: home.path().into(),
            config_dir: home.path().join("config"),
            runner: &runner,
            detached_ticker: false,
        };
        let pane = crate::herdr::Pane {
            pane_id: "w1:p1".into(),
            tab_id: "w1:t1".into(),
            workspace_id: "w1".into(),
            cwd,
            terminal_id: "term-1".into(),
            ..Default::default()
        };
        let current = Current {
            socket: "/managed.sock".into(),
            pane_id: "w1:p1".into(),
            terminal_id: "term-1".into(),
            agent: "claude".into(),
            pane,
        };
        assert!(matches!(managed(&ctx, &current), Some(Seat::Root(_))));
    }

    #[test]
    fn activity_is_cleaned_and_capped_at_forty_columns() {
        assert_eq!(clean("  Reading code\n", 40), "Reading code");
        assert_eq!(clean(&"x".repeat(60), 40).len(), 40);
        assert_eq!(clean("a\u{202e}b\u{7}c", 40), "abc");
        assert_eq!(clean("日本語テキスト", 6), "日本語");
    }

    #[test]
    fn helpers_and_the_reporters_own_call_do_not_trigger() {
        assert!(!eligible(
            &json!({"hook_event_name":"PostToolUse","agent_id":"child"})
        ));
        assert!(!eligible(
            &json!({"hook_event_name":"PostToolUse","transcript_path":"/x/subagents/y.jsonl"})
        ));
        assert!(!eligible(
            &json!({"hook_event_name":"PostToolUse","tool_input":{"command":"/p/herdr-projects --root /r report --percent 5 --activity x"}})
        ));
        assert!(eligible(
            &json!({"hook_event_name":"PostToolUse","tool_input":{"command":"/p/herdr-projects --root /r context demo"}})
        ));
        assert!(eligible(
            &json!({"hook_event_name":"SessionStart","source":"compact"})
        ));
        assert!(!eligible(&json!({"hook_event_name":"Stop"})));
    }

    #[test]
    fn session_start_injects_instructions_and_resets_the_record() {
        let mut record = Record {
            pane_id: "w1:p1".into(),
            activity: "Old".into(),
            percent: Some(50),
            reported_at: 5,
            ..Record::default()
        };
        let text = respond(
            &mut record,
            &event("SessionStart"),
            PaneRole::Coordinator,
            "/p/hp --root /r",
            1000,
        )
        .unwrap();
        assert!(text.contains("/p/hp --root /r report --percent 25"));
        assert!(text.contains("(w1:p1)"));
        assert_eq!(record.activity, "");
        assert_eq!(record.percent, None);
        assert_eq!(record.reported_at, 0);
        assert_eq!(record.session_started_at, 1000);
    }

    fn event(kind: &str) -> serde_json::Value {
        json!({ "hook_event_name": kind })
    }

    fn prompt(text: &str) -> serde_json::Value {
        json!({ "hook_event_name": "UserPromptSubmit", "prompt": text })
    }

    #[test]
    fn a_worker_gets_at_most_one_check_in_every_five_minutes_and_none_at_done() {
        let tool = event("PostToolUse");
        let worker =
            |record: &mut Record, now: i64| respond(record, &tool, PaneRole::Worker, "hp", now);
        let mut record = Record {
            reminded_at: 1000,
            ..Record::default()
        };
        assert!(worker(&mut record, 1060).is_none());
        assert!(worker(&mut record, 1299).is_none());
        assert!(worker(&mut record, 1300).is_some());
        assert_eq!(record.reminded_at, 1300);
        assert!(worker(&mut record, 1301).is_none());
        // A fresh report also quiets the check-in for five minutes.
        record.reported_at = 1500;
        assert!(worker(&mut record, 1700).is_none());
        assert!(worker(&mut record, 1800).is_some());
        record.percent = Some(100);
        assert!(worker(&mut record, 9000).is_none());
        assert!(
            respond(
                &mut record,
                &prompt("Next task"),
                PaneRole::Worker,
                "hp",
                9001
            )
            .unwrap()
            .contains("Waiting for you")
        );
        let mut asked = Record {
            activity: WAITING.into(),
            percent: Some(40),
            reported_at: 5,
            ..Record::default()
        };
        respond(&mut asked, &prompt("Yes"), PaneRole::Worker, "hp", 10);
        assert!(!asked.waiting(), "an answer clears the old question");
        assert!(respond(&mut record, &event("Stop"), PaneRole::Worker, "hp", 9002).is_none());
    }

    #[test]
    fn coordinator_panes_get_no_check_in_but_a_human_prompt_still_gets_its_reminder() {
        let mut record = Record::default();
        for now in [1000, 5000, 100_000] {
            assert!(
                respond(
                    &mut record,
                    &event("PostToolUse"),
                    PaneRole::Coordinator,
                    "hp",
                    now
                )
                .is_none()
            );
        }
        assert!(
            respond(
                &mut record,
                &prompt("Start the release"),
                PaneRole::Coordinator,
                "hp",
                100_001
            )
            .unwrap()
            .contains("report a fresh estimate")
        );
    }

    #[test]
    fn automated_prompts_get_no_reminder_and_leave_the_record_alone() {
        for text in [
            "[hp inbox] t-0040 new report",
            "[hp ticker] old nudge",
            "[hp routine pr-followup] Your pull request https://github.com/o/r/pull/4 changed: it was merged.\n\nClean up.",
            "[herdr-projects ticker: automated, not the user, approves nothing] t-0094 ready for review. Inspect with `hp node show demo <id>`.",
            "  <task-notification>\n<task-id>b1</task-id>",
            "[from the root coordinator] Check the login.",
            "[from t-0093 \"Mi Cúcula\"] Go.",
        ] {
            for role in [PaneRole::Coordinator, PaneRole::Worker] {
                let mut record = Record {
                    activity: WAITING.into(),
                    reported_at: 5,
                    reminded_at: 7,
                    ..Record::default()
                };
                let before = record.clone();
                assert!(
                    respond(&mut record, &prompt(text), role, "hp", 1000).is_none(),
                    "{text}"
                );
                assert_eq!(record, before, "{text}: still waiting for the user");
            }
        }
        // Text that only mentions a prefix later on is the user's.
        assert!(!automated("what does [hp inbox] mean?"));
        assert!(automated("[hp inbox] t-0001 new report"));
    }

    /// The ticker types a thread's brief line. It is not the user's message:
    /// it gets no reminder on top of the brief, and a coordinator node's first
    /// real message is not checked against it.
    #[test]
    fn a_brief_line_is_not_the_users_message() {
        let home = tempfile::tempdir().unwrap();
        let project = crate::project::create(home.path(), "demo", "", vec![]).unwrap();
        let node = crate::thread::Thread {
            id: "t-0001".into(),
            role: crate::thread::NodeRole::Coordinator,
            can_spawn: true,
            ..Default::default()
        };
        let seat = Seat::Node(project.clone(), Box::new(node));
        let brief = prompt(&crate::thread::launch_prompt("demo", "t-0001"));
        let mut record = Record::default();
        assert!(respond(&mut record, &brief, PaneRole::Coordinator, "hp", 100).is_none());
        assert_eq!(note_human_prompt(&seat, &brief, 100), None);
        assert_eq!(
            note_human_prompt(&seat, &prompt("Plan the checkout"), 200),
            None
        );
        let previous = crate::status_file::previous_ask(&project, "t-0001");
        assert_eq!(previous, None, "one message from the user so far");
    }

    /// Another coordinator's signed message is not the user's: it does not
    /// move the time a coordinator's STATUS.md is checked against.
    #[test]
    fn a_signed_prompt_is_not_the_users_last_message() {
        let home = tempfile::tempdir().unwrap();
        let project = crate::project::create(home.path(), "demo", "", vec![]).unwrap();
        let seat = Seat::Root(project.clone());
        note_human_prompt(&seat, &prompt("Ship it"), 100);
        note_human_prompt(&seat, &prompt("[from t-0093 \"Docs\"] Done."), 200);
        note_human_prompt(&seat, &prompt("And the docs"), 300);
        let previous = crate::status_file::previous_ask(&project, crate::status_file::ROOT_KEY);
        assert_eq!(previous, Some(100));
    }

    /// A coordinator node whose STATUS.md is older than the user's previous
    /// message, in a project with the given settings.
    fn stale_node(settings: &crate::project::Settings) -> (tempfile::TempDir, Seat) {
        let home = tempfile::tempdir().unwrap();
        let project = crate::project::create(home.path(), "demo", "", vec![]).unwrap();
        let mut text = std::fs::read_to_string(project.project_md()).unwrap();
        for (key, on) in [
            ("progress", settings.progress),
            ("turn_reminder", settings.turn_reminder),
        ] {
            text = text.replacen("+++\n", &format!("+++\n{key} = {on}\n"), 1);
        }
        std::fs::write(project.project_md(), text).unwrap();
        let node = crate::thread::Thread {
            id: "t-0001".into(),
            role: crate::thread::NodeRole::Coordinator,
            can_spawn: true,
            ..Default::default()
        };
        crate::status_file::record_ask(&project, "t-0001", 1).unwrap();
        (home, Seat::Node(project, Box::new(node)))
    }

    fn settings(progress: bool, turn_reminder: bool) -> crate::project::Settings {
        crate::project::Settings {
            progress,
            turn_reminder,
            ..Default::default()
        }
    }

    /// What every event answers in a project that configures neither switch:
    /// the same text the hook gave before they existed.
    #[test]
    fn with_both_switches_on_the_hook_says_what_it_always_said() {
        let on = settings(true, true);
        let (_home, seat) = stale_node(&on);
        let mut record = Record {
            pane_id: "w1:p1".into(),
            ..Record::default()
        };
        assert_eq!(
            answer(&seat, &on, &mut record, &event("SessionStart"), "hp", 1000),
            Some(instructions("hp", "w1:p1"))
        );
        assert_eq!(
            answer(&seat, &on, &mut record, &prompt("Plan it"), "hp", 2000),
            Some(format!(
                "Before task tools or a blocking question, check whether this request starts new work; if so report a fresh estimate. Then report before waiting for the user, for example `hp report --unknown --activity 'Waiting for you'`.\n\n{}",
                crate::status_file::NODE_STALE
            ))
        );
        let worker = Seat::Root(seat.project().clone());
        let mut record = Record::default();
        assert_eq!(
            answer(&worker, &on, &mut record, &event("PostToolUse"), "hp", 9000),
            None,
            "the root gets no check-in"
        );
        let node = crate::thread::Thread::default();
        let worker = Seat::Node(seat.project().clone(), Box::new(node));
        assert_eq!(
            answer(&worker, &on, &mut record, &event("PostToolUse"), "hp", 9000),
            Some(reminder("hp"))
        );
    }

    /// `progress = false`: no instructions, no reminder, no check-in, in any
    /// pane of the project. The stale status line is not progress.
    #[test]
    fn progress_off_gives_no_progress_text_anywhere() {
        let off = settings(false, true);
        let (_home, seat) = stale_node(&off);
        let mut record = Record::default();
        assert_eq!(
            answer(&seat, &off, &mut record, &event("SessionStart"), "hp", 1000),
            None
        );
        assert_eq!(
            answer(&seat, &off, &mut record, &prompt("Plan it"), "hp", 2000),
            Some(crate::status_file::NODE_STALE.to_string())
        );
        for seat in [
            Seat::Root(seat.project().clone()),
            Seat::Node(seat.project().clone(), Box::default()),
        ] {
            let mut record = Record::default();
            for kind in ["SessionStart", "PostToolUse"] {
                assert_eq!(
                    answer(&seat, &off, &mut record, &event(kind), "hp", 9000),
                    None,
                    "{kind}"
                );
            }
            assert_eq!(
                answer(&seat, &off, &mut record, &prompt("Go"), "hp", 9001),
                None
            );
        }
    }

    /// `turn_reminder = false`: nothing after a prompt the user typed, while
    /// the time of the prompt is still recorded for `status-stale`.
    #[test]
    fn turn_reminder_off_adds_nothing_after_a_typed_prompt_and_still_records_it() {
        let off = settings(true, false);
        let (_home, seat) = stale_node(&off);
        let mut record = Record {
            pane_id: "w1:p1".into(),
            ..Record::default()
        };
        assert_eq!(
            answer(&seat, &off, &mut record, &prompt("Plan it"), "hp", 2000),
            None
        );
        assert_eq!(
            crate::status_file::previous_ask(seat.project(), "t-0001"),
            Some(1),
            "the prompt was recorded"
        );
        assert_eq!(
            answer(&seat, &off, &mut record, &event("SessionStart"), "hp", 3000),
            Some(instructions("hp", "w1:p1")),
            "progress itself stays on"
        );
        let root = Seat::Root(seat.project().clone());
        assert_eq!(
            answer(&root, &off, &mut record, &prompt("Next"), "hp", 4000),
            None
        );
        assert_eq!(
            crate::status_file::previous_ask(seat.project(), crate::status_file::ROOT_KEY),
            None
        );
        answer(&root, &off, &mut record, &prompt("Again"), "hp", 5000);
        assert_eq!(
            crate::status_file::previous_ask(seat.project(), crate::status_file::ROOT_KEY),
            Some(4000)
        );
    }

    #[test]
    fn native_events_are_normalized_and_answered_in_each_harness_shape() {
        let gemini = crate::setup::harness("gemini").unwrap();
        let event = normalize(
            gemini,
            json!({"hook_event_name":"AfterTool","tool_input":{"command":"ls"}}),
        );
        assert_eq!(event["hook_event_name"], "PostToolUse");
        assert!(eligible(&event));
        assert_eq!(
            normalize(gemini, json!({"hook_event_name":"BeforeAgent"}))["hook_event_name"],
            "UserPromptSubmit"
        );
        assert_eq!(
            output(gemini, "AfterTool", "hi"),
            json!({"hookSpecificOutput":{"hookEventName":"AfterTool","additionalContext":"hi"}})
        );
        let copilot = crate::setup::harness("copilot").unwrap();
        assert_eq!(
            output(copilot, "SessionStart", "hi"),
            json!({"additionalContext":"hi"})
        );
    }

    #[test]
    fn events_from_another_session_in_the_pane_are_a_subagents() {
        let record = Record {
            session_id: "main".into(),
            ..Record::default()
        };
        assert!(foreign_session(&record, &json!({"session_id":"sub"})));
        assert!(!foreign_session(&record, &json!({"session_id":"main"})));
        assert!(!foreign_session(&record, &json!({})));
        assert!(
            !foreign_session(&Record::default(), &json!({"session_id":"sub"})),
            "no SessionStart seen: accept"
        );
    }

    #[test]
    fn guidance_names_the_command_and_the_pane_when_known() {
        assert!(guidance("/p/hp --root /r", None).contains("top-level agent in this pane reports"));
        assert!(guidance("/p/hp --root /r", Some("w2:p3")).contains("this pane (w2:p3)"));
        assert!(
            instructions("hp", "w1:p1")
                .starts_with("# Progress (herdr-projects)\n\nReport the progress")
        );
    }

    #[test]
    fn records_round_trip_per_session_and_stale_terminals_are_ignored() {
        let root = tempfile::tempdir().unwrap();
        let record = Record {
            socket: "/a.sock".into(),
            pane_id: "w1:p1".into(),
            terminal_id: "term_1".into(),
            activity: WAITING.into(),
            reported_at: 7,
            ..Record::default()
        };
        save(root.path(), &record).unwrap();
        let other = Record {
            socket: "/b.sock".into(),
            pane_id: "w1:p1".into(),
            terminal_id: "term_9".into(),
            activity: "Testing".into(),
            reported_at: 8,
            ..Record::default()
        };
        save(root.path(), &other).unwrap();
        assert_eq!(load(root.path(), "/a.sock", "w1:p1").unwrap(), record);
        assert!(
            self_report(root.path(), "/a.sock", "w1:p1", "term_1")
                .unwrap()
                .waiting()
        );
        assert!(self_report(root.path(), "/a.sock", "w1:p1", "term_2").is_none());
        assert!(self_report(root.path(), "/a.sock", "w1:p1", "").is_some());
        prune(root.path(), "/a.sock", &["w1:p2".into()]);
        assert!(load(root.path(), "/a.sock", "w1:p1").is_none());
        assert!(load(root.path(), "/b.sock", "w1:p1").is_some());
    }
}

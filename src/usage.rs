//! Token use per local node: the root coordinator, each coordinator node and
//! each thread. Herdr reports no usage, only each pane's native session id
//! (`Agent::session_id`). The ticker keeps every id a node's pane reported in
//! its record (`sessions`), and this module reads the harness's own
//! transcripts for them: Claude Code's `<config>/projects/<cwd>/<id>.jsonl`,
//! with its subagents in `<id>/subagents/*.jsonl`, and Codex's
//! `sessions/**/rollout-*-<id>.jsonl`.
//!
//! A read takes only the lines added since the last one: `.state/usage.json`
//! keeps each file's offset and counts, so the totals outlive a transcript the
//! harness deletes. A line it does not understand is skipped. Only numbers are
//! kept; no transcript text is stored, printed or sent anywhere.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::notify::{Notifier, Sound};
use crate::organizations::ROOT_ID;
use crate::paths::Ctx;
use crate::project::{self, Project};
use crate::thread;

/// The ticker reads transcripts at most this often, and whenever a node
/// stops working.
pub const REFRESH_SECS: i64 = 300;

pub const CLAUDE: &str = "claude";
pub const CODEX: &str = "codex";

/// `.state/usage.json`.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(default)]
pub struct Store {
    /// When every open local node was last read.
    pub refreshed: String,
    /// `root` or a thread id.
    pub nodes: BTreeMap<String, Node>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(default)]
pub struct Node {
    /// The caps it was at or above at its last read, so a crossing notifies once.
    pub over: Vec<String>,
    /// Transcript path to what was read from it.
    pub files: BTreeMap<String, File>,
}

/// What was read from one transcript.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(default)]
pub struct File {
    pub harness: String,
    pub session: String,
    /// A Claude Code subagent's transcript: its tokens count, its context
    /// and compactions are its own.
    pub subagent: bool,
    /// Bytes read, always at the end of a line.
    pub offset: u64,
    /// Claude Code writes a request once per content block, the rows one after
    /// another: the last `message.id`/`requestId` counted.
    pub last_key: String,
    /// Codex: the last cumulative input, cached input and output.
    pub cumulative: [u64; 3],
    pub requests: u64,
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    /// The last request's prompt: input, cache read and cache write.
    pub context: u64,
    pub peak_context: u64,
    pub compactions: u64,
    /// When the last counted request was made.
    pub last_at: String,
}

impl File {
    fn new(harness: &str, session: &str, subagent: bool) -> File {
        File {
            harness: harness.into(),
            session: session.into(),
            subagent,
            ..File::default()
        }
    }

    fn count(&mut self, input: u64, output: u64, read: u64, write: u64, context: u64, at: &str) {
        self.requests += 1;
        self.input += input;
        self.output += output;
        self.cache_read += read;
        self.cache_write += write;
        self.context = context;
        self.peak_context = self.peak_context.max(context);
        if let Some(at) = instant(at) {
            self.last_at = at;
        }
    }
}

/// A node's use in the `--json` contract.
#[derive(Debug, Clone, Serialize, Default, PartialEq)]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub context: u64,
    pub peak_context: u64,
    pub compactions: u64,
    pub updated: String,
    pub over_cap: Vec<String>,
}

impl Usage {
    /// What `tokens_warn` counts: cache reads repeat the context on every
    /// request and would dwarf the rest.
    fn tokens(&self) -> u64 {
        self.input + self.cache_write + self.output
    }
}

impl Node {
    /// What its transcripts add up to: tokens from every file, context and
    /// compactions from its own sessions, the context of the latest.
    /// `None` until one request or compaction was understood.
    pub fn usage(&self) -> Option<Usage> {
        let files: Vec<&File> = self
            .files
            .values()
            .filter(|f| f.requests > 0 || f.compactions > 0)
            .collect();
        if files.is_empty() {
            return None;
        }
        let own = || files.iter().filter(|f| !f.subagent);
        let sum = |field: fn(&File) -> u64| files.iter().map(|f| field(f)).sum();
        Some(Usage {
            input: sum(|f| f.input),
            output: sum(|f| f.output),
            cache_read: sum(|f| f.cache_read),
            cache_write: sum(|f| f.cache_write),
            context: own()
                .max_by(|a, b| a.last_at.cmp(&b.last_at))
                .map_or(0, |f| f.context),
            peak_context: own().map(|f| f.peak_context).max().unwrap_or(0),
            compactions: own().map(|f| f.compactions).sum(),
            updated: files
                .iter()
                .map(|f| f.last_at.clone())
                .max()
                .unwrap_or_default(),
            over_cap: self.over.clone(),
        })
    }
}

impl Store {
    pub fn usage(&self, id: &str) -> Option<Usage> {
        self.nodes.get(id).and_then(Node::usage)
    }

    /// The caps node `id` was at or above at its last read.
    pub fn over(&self, id: &str) -> Vec<String> {
        self.nodes
            .get(id)
            .map(|n| n.over.clone())
            .unwrap_or_default()
    }
}

/// `[usage."<project folder>"]`, then `[usage.default]`, in config.toml.
/// Zero is no cap.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Caps {
    pub context_warn: u64,
    pub compaction_warn: u64,
    pub tokens_warn: u64,
}

/// A cap `usage` is at or above: its config.toml key and the notification.
struct Reached {
    key: &'static str,
    text: String,
}

impl Caps {
    fn reached(&self, usage: &Usage) -> Vec<Reached> {
        [
            ("context_warn", self.context_warn, usage.context),
            ("compaction_warn", self.compaction_warn, usage.compactions),
            ("tokens_warn", self.tokens_warn, usage.tokens()),
        ]
        .into_iter()
        .filter(|(_, cap, value)| *cap > 0 && value >= cap)
        .map(|(key, cap, value)| {
            let what = match key {
                "context_warn" => format!("context {} tokens", short(value)),
                "compaction_warn" => plural(value, "compaction"),
                _ => format!("{} tokens used", short(value)),
            };
            Reached {
                key,
                text: format!("{what}, at or above its cap of {}", short(cap)),
            }
        })
        .collect()
    }
}

pub fn path(project: &Project) -> PathBuf {
    project.state_dir().join("usage.json")
}

pub fn load(project: &Project) -> Store {
    project::read_json(&path(project)).unwrap_or_default()
}

/// Reads what every open local node's transcripts gained since the last
/// read, records the caps each node is at or above, and notifies each cap a
/// node newly reached, once and without sound.
pub fn refresh(ctx: &Ctx, project: &Project) -> Result<Store> {
    let caps = project.usage_caps(&ctx.config_dir)?;
    let mut reached = Vec::new();
    let store = {
        let _lock = lock(project)?;
        let mut store = load(project);
        for node in local_nodes(project) {
            let mut entry = store.nodes.remove(&node.id).unwrap_or_default();
            read_node(ctx, &node, &mut entry);
            let now = entry.usage().map(|u| caps.reached(&u)).unwrap_or_default();
            for cap in now
                .iter()
                .filter(|c| !entry.over.iter().any(|k| k == c.key))
            {
                reached.push((node.id.clone(), cap.text.clone()));
            }
            entry.over = now.iter().map(|c| c.key.to_string()).collect();
            if !entry.files.is_empty() {
                store.nodes.insert(node.id, entry);
            }
        }
        store.refreshed = project::now();
        project::write_json(&path(project), &store)?;
        store
    };
    if !reached.is_empty() {
        let notifier = Notifier::new(ctx, project);
        for (id, body) in reached {
            let subject = if id == ROOT_ID { "coordinator" } else { &id };
            notifier.send(subject, &body, Sound::None, false);
        }
    }
    Ok(store)
}

/// The ticker's pass: a read at most every five minutes, and at once when a
/// node stopped working, so a finished turn shows without waiting.
pub fn tick(ctx: &Ctx, project: &Project, stopped: bool, now: jiff::Timestamp) -> Result<()> {
    let due = load(project)
        .refreshed
        .parse::<jiff::Timestamp>()
        .map_or(true, |at| now.duration_since(at).as_secs() >= REFRESH_SECS);
    if stopped || due {
        refresh(ctx, project)?;
    }
    Ok(())
}

/// `node usage <slug> [--json]`: reads the transcripts now, then one row per
/// node, the root first, in tree order.
pub fn print(ctx: &Ctx, slug: &str, json: bool) -> Result<()> {
    let project = Project::load(&ctx.root, slug)?;
    let store = refresh(ctx, &project)?;
    let mut rows = vec![(
        ROOT_ID.to_string(),
        "Coordinator".to_string(),
        String::new(),
    )];
    rows.extend(
        crate::organizations::tree(&project)?
            .into_iter()
            .map(|e| (e.thread.id, e.thread.title, e.thread.machine)),
    );
    if json {
        let nodes: Vec<serde_json::Value> = rows
            .iter()
            .map(|(id, title, _)| serde_json::json!({"id": id, "title": title, "usage": store.usage(id)}))
            .collect();
        return crate::threads::print_json(&serde_json::json!({
            "schema_version": crate::threads::JSON_SCHEMA_VERSION,
            "project": project.slug,
            "nodes": nodes,
        }));
    }
    for (id, title, machine) in rows {
        let line = match store.usage(&id) {
            Some(u) => {
                let mut line = format!(
                    "context {} · peak {} · {} · {} tokens · {} cache read",
                    short(u.context),
                    short(u.peak_context),
                    plural(u.compactions, "compaction"),
                    short(u.tokens()),
                    short(u.cache_read)
                );
                if !u.over_cap.is_empty() {
                    line.push_str(&format!(" · over {}", u.over_cap.join(", ")));
                }
                line
            }
            None if !machine.is_empty() => format!("on {machine}, not read"),
            None => "no data".into(),
        };
        println!("{id}\t{title}\t{line}");
    }
    Ok(())
}

/// The CLI and the ticker read one at a time, so a crossing notifies once.
/// Not the project lock: a first read of a long transcript takes a while,
/// and nothing else needs to wait for it.
fn lock(project: &Project) -> Result<std::fs::File> {
    let file = std::fs::File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(project.state_dir().join("usage.lock"))?;
    file.lock()?;
    Ok(file)
}

/// A node whose transcripts are on this machine.
struct Local {
    id: String,
    harness: String,
    cwd: String,
    sessions: Vec<String>,
}

/// The root coordinator and every open local thread. A resolved thread was
/// read when it stopped; a remote one's transcripts are on its machine.
fn local_nodes(project: &Project) -> Vec<Local> {
    let root = project.coordinator().map(|c| Local {
        id: ROOT_ID.into(),
        harness: c.agent,
        cwd: if c.cwd.is_empty() {
            project.canonical_dir().to_string_lossy().into_owned()
        } else {
            c.cwd
        },
        sessions: c.sessions,
    });
    let threads = thread::list(project)
        .into_iter()
        .filter(|t| t.status != thread::Status::Resolved && !t.is_remote())
        .map(|t| Local {
            id: t.id,
            harness: t.agent,
            cwd: t.cwd,
            sessions: t.sessions,
        });
    root.into_iter().chain(threads).collect()
}

/// Finds the transcripts of sessions new to `entry`, and of Claude Code
/// subagents, then reads what every file gained.
fn read_node(ctx: &Ctx, node: &Local, entry: &mut Node) {
    for session in &node.sessions {
        let known = entry
            .files
            .values()
            .any(|f| f.session == *session && !f.subagent);
        if !known && let Some(path) = locate(ctx, &node.harness, &node.cwd, session) {
            entry
                .files
                .entry(path.to_string_lossy().into_owned())
                .or_insert_with(|| File::new(&node.harness, session, false));
        }
    }
    let mains: Vec<(String, String)> = entry
        .files
        .iter()
        .filter(|(_, f)| f.harness == CLAUDE && !f.subagent)
        .map(|(path, f)| (path.clone(), f.session.clone()))
        .collect();
    for (main, session) in mains {
        for path in subagents(Path::new(&main)) {
            entry
                .files
                .entry(path.to_string_lossy().into_owned())
                .or_insert_with(|| File::new(CLAUDE, &session, true));
        }
    }
    for (path, file) in &mut entry.files {
        read(Path::new(path), file);
    }
}

/// The transcript of `session` for a node on `harness` working in `cwd`.
fn locate(ctx: &Ctx, harness: &str, cwd: &str, session: &str) -> Option<PathBuf> {
    // Herdr may name the transcript itself (`kind: path`).
    let named = Path::new(session);
    if named.is_absolute() {
        return named.is_file().then(|| named.to_path_buf());
    }
    if session.is_empty() || session.starts_with('.') || session.contains(['/', '\\']) {
        return None;
    }
    let file = format!("{session}.jsonl");
    match harness {
        CLAUDE => {
            let own = crate::rename::claude_projects(ctx, Path::new(cwd));
            if own.join(&file).is_file() {
                return Some(own.join(&file));
            }
            // Claude Code shortens a long folder name, and a link may name
            // the folder another way: look in every one.
            std::fs::read_dir(own.parent()?)
                .ok()?
                .flatten()
                .map(|e| e.path().join(&file))
                .find(|p| p.is_file())
        }
        CODEX => {
            let home = ctx
                .env
                .var("CODEX_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| ctx.env.home.join(".codex"));
            rollout(&home.join("sessions"), &format!("-{file}"), 4)
        }
        _ => None,
    }
}

/// `rollout-<time>-<id>.jsonl` under Codex's dated session folders.
fn rollout(dir: &Path, suffix: &str, depth: usize) -> Option<PathBuf> {
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let name = entry.file_name().to_string_lossy().into_owned();
        if kind.is_dir() && depth > 0 {
            if let Some(found) = rollout(&entry.path(), suffix, depth - 1) {
                return Some(found);
            }
        } else if kind.is_file() && name.starts_with("rollout-") && name.ends_with(suffix) {
            return Some(entry.path());
        }
    }
    None
}

/// The subagent transcripts Claude Code keeps beside the session in `main`.
fn subagents(main: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(main.with_extension("").join("subagents")) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "jsonl"))
        .collect()
}

/// Reads the lines `file` gained since its offset. A missing transcript
/// keeps what was read from it (the harness deletes old ones); a shorter one
/// was rewritten and is read again from its start.
fn read(path: &Path, file: &mut File) {
    let Ok(meta) = std::fs::metadata(path) else {
        return;
    };
    if !meta.is_file() {
        return;
    }
    if meta.len() < file.offset {
        *file = File::new(&file.harness, &file.session, file.subagent);
    }
    if meta.len() == file.offset {
        return;
    }
    let Ok(mut handle) = std::fs::File::open(path) else {
        return;
    };
    if handle.seek(SeekFrom::Start(file.offset)).is_err() {
        return;
    }
    let mut reader = BufReader::with_capacity(1 << 16, handle);
    let mut line = Vec::new();
    loop {
        line.clear();
        match reader.read_until(b'\n', &mut line) {
            Ok(n) if n > 0 && line.ends_with(b"\n") => {
                file.offset += n as u64;
                match file.harness.as_str() {
                    CLAUDE => claude_line(&line, file),
                    CODEX => codex_line(&line, file),
                    _ => {}
                }
            }
            // The end, or a line the harness is still writing.
            _ => break,
        }
    }
}

#[derive(Deserialize)]
struct ClaudeRow {
    #[serde(rename = "type", default)]
    kind: String,
    #[serde(default)]
    subtype: Option<String>,
    #[serde(rename = "requestId", default)]
    request_id: Option<String>,
    #[serde(default)]
    timestamp: Option<String>,
    #[serde(default)]
    message: Option<ClaudeMessage>,
}

#[derive(Deserialize)]
struct ClaudeMessage {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    usage: Option<ClaudeUsage>,
}

#[derive(Deserialize)]
struct ClaudeUsage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    cache_read_input_tokens: u64,
    #[serde(default)]
    cache_creation_input_tokens: u64,
}

fn claude_line(line: &[u8], file: &mut File) {
    // Most rows are neither, and are not parsed.
    if !has(line, b"\"usage\"") && !has(line, b"compact_boundary") {
        return;
    }
    let Ok(row) = serde_json::from_slice::<ClaudeRow>(line) else {
        return;
    };
    if row.kind == "system" && row.subtype.as_deref() == Some("compact_boundary") {
        file.compactions += 1;
        return;
    }
    let Some(message) = row.message.filter(|_| row.kind == "assistant") else {
        return;
    };
    // Claude Code's own notices are no request; their counts are zero.
    if message.model.as_deref() == Some("<synthetic>") {
        return;
    }
    let Some(usage) = message.usage else {
        return;
    };
    let key = format!(
        "{}/{}",
        message.id.unwrap_or_default(),
        row.request_id.unwrap_or_default()
    );
    if key == file.last_key {
        return;
    }
    file.last_key = key;
    let context =
        usage.input_tokens + usage.cache_read_input_tokens + usage.cache_creation_input_tokens;
    file.count(
        usage.input_tokens,
        usage.output_tokens,
        usage.cache_read_input_tokens,
        usage.cache_creation_input_tokens,
        context,
        row.timestamp.as_deref().unwrap_or(""),
    );
}

#[derive(Deserialize)]
struct CodexRow {
    #[serde(default)]
    timestamp: Option<String>,
    #[serde(default)]
    payload: Option<CodexPayload>,
}

#[derive(Deserialize)]
struct CodexPayload {
    #[serde(rename = "type", default)]
    kind: String,
    #[serde(default)]
    info: Option<CodexInfo>,
}

#[derive(Deserialize)]
struct CodexInfo {
    #[serde(default)]
    total_token_usage: Option<CodexTokens>,
    #[serde(default)]
    last_token_usage: Option<CodexTokens>,
}

#[derive(Deserialize, Default, Clone, Copy)]
#[serde(default)]
struct CodexTokens {
    input_tokens: u64,
    cached_input_tokens: u64,
    output_tokens: u64,
}

impl CodexTokens {
    fn columns(self) -> [u64; 3] {
        [
            self.input_tokens,
            self.cached_input_tokens,
            self.output_tokens,
        ]
    }
}

/// Codex writes cumulative totals: a request is what they grew by, its input
/// split into the uncached part and the cache read.
fn codex_line(line: &[u8], file: &mut File) {
    if !has(line, b"token_count") {
        return;
    }
    let Ok(row) = serde_json::from_slice::<CodexRow>(line) else {
        return;
    };
    let Some(info) = row
        .payload
        .filter(|p| p.kind == "token_count")
        .and_then(|p| p.info)
    else {
        return;
    };
    let Some(total) = info.total_token_usage else {
        return;
    };
    let now = total.columns();
    let last = info.last_token_usage.unwrap_or_default();
    let grown = [0, 1, 2].map(|i| now[i].checked_sub(file.cumulative[i]));
    let delta = if grown.iter().all(Option::is_some) {
        grown.map(Option::unwrap_or_default)
    } else {
        // A total that went down was reset by a compaction or a resume: the
        // event's own use is its last request's.
        file.compactions += 1;
        last.columns()
    };
    file.cumulative = now;
    // Codex repeats an unchanged total.
    if delta == [0, 0, 0] {
        return;
    }
    file.count(
        delta[0].saturating_sub(delta[1]),
        delta[2],
        delta[1],
        0,
        last.input_tokens,
        row.timestamp.as_deref().unwrap_or(""),
    );
}

fn has(line: &[u8], needle: &[u8]) -> bool {
    line.windows(needle.len()).any(|w| w == needle)
}

/// A transcript's time as the JSON contract writes times, to the second.
fn instant(text: &str) -> Option<String> {
    let at = text.parse::<jiff::Timestamp>().ok()?;
    at.round(jiff::Unit::Second).ok().map(|t| t.to_string())
}

/// 1300 as `1.3k`, 1,000,300 as `1M`.
fn short(n: u64) -> String {
    let (value, unit) = match n {
        0..1_000 => return n.to_string(),
        1_000..1_000_000 => (n as f64 / 1e3, "k"),
        _ => (n as f64 / 1e6, "M"),
    };
    let text = format!("{value:.1}");
    format!("{}{unit}", text.strip_suffix(".0").unwrap_or(&text))
}

fn plural(n: u64, word: &str) -> String {
    if n == 1 {
        format!("1 {word}")
    } else {
        format!("{n} {word}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::fake::ok;
    use crate::scenarios::World;
    use serde_json::json;
    use std::io::Write;
    use std::path::Path;

    fn assistant(id: &str, at: &str, input: u64, output: u64, read: u64, write: u64) -> String {
        json!({
            "type": "assistant", "timestamp": at, "requestId": format!("req-{id}"), "sessionId": "s",
            "message": {"id": id, "role": "assistant",
                "content": [{"type": "text", "text": "synthetic"}],
                "usage": {"input_tokens": input, "output_tokens": output,
                    "cache_read_input_tokens": read, "cache_creation_input_tokens": write}}
        })
        .to_string()
    }

    /// Claude Code's own notice after an interrupted turn: no request.
    fn synthetic() -> String {
        json!({"type": "assistant", "timestamp": "2026-10-16T10:03:00Z", "requestId": "req-n",
            "message": {"id": "n", "model": "<synthetic>", "content": [],
                "usage": {"input_tokens": 0, "output_tokens": 0,
                    "cache_read_input_tokens": 0, "cache_creation_input_tokens": 0}}})
        .to_string()
    }

    fn compaction() -> String {
        json!({"type": "system", "subtype": "compact_boundary",
            "compactMetadata": {"trigger": "auto", "preTokens": 900, "postTokens": 100}})
        .to_string()
    }

    fn user() -> String {
        json!({"type": "user", "message": {"role": "user", "content": "synthetic \"usage\" prompt"}})
            .to_string()
    }

    fn token_count(at: &str, total: [u64; 3], last: [u64; 3]) -> String {
        let cols = |c: [u64; 3]| {
            json!({"input_tokens": c[0], "cached_input_tokens": c[1],
            "output_tokens": c[2], "reasoning_output_tokens": 0, "total_tokens": c[0] + c[2]})
        };
        json!({"timestamp": at, "type": "event_msg",
            "payload": {"type": "token_count", "info": {"total_token_usage": cols(total),
                "last_token_usage": cols(last), "model_context_window": 272000}}})
        .to_string()
    }

    fn append(path: &Path, lines: &[String]) {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        for line in lines {
            writeln!(file, "{line}").unwrap();
        }
    }

    /// `<home>/.claude/projects/<cwd as Claude Code names it>/<session>.jsonl`.
    fn claude_transcript(world: &World, cwd: &str, session: &str) -> PathBuf {
        let folder: String = cwd
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect();
        let dir = world.home.path().join(".claude/projects").join(folder);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(format!("{session}.jsonl"))
    }

    /// The demo project's coordinator runs Claude Code in session `root-1`.
    fn demo(world: &World) -> (Project, PathBuf) {
        let project = world.project("demo", "a.sock");
        let record = project
            .update_coordinator(|c| {
                c.agent = CLAUDE.into();
                c.sessions = vec!["root-1".into()];
            })
            .unwrap();
        let transcript = claude_transcript(world, &record.cwd, "root-1");
        (project, transcript)
    }

    fn root(world: &World, project: &Project) -> Option<Usage> {
        refresh(&world.ctx(), project)
            .unwrap()
            .usage(crate::organizations::ROOT_ID)
    }

    /// Claude Code repeats a request's row once per content block: each
    /// request counts once, the context is the last request's prompt, and a
    /// line that is not JSON and a notice Claude Code wrote itself are skipped.
    #[test]
    fn a_transcript_counts_each_request_once_with_its_context_peak_and_compactions() {
        let world = World::new();
        let (project, transcript) = demo(&world);
        append(
            &transcript,
            &[
                user(),
                assistant("m1", "2026-10-16T10:00:00.100Z", 10, 5, 100, 50),
                assistant("m1", "2026-10-16T10:00:00.100Z", 10, 5, 100, 50),
                assistant("m2", "2026-10-16T10:01:00.000Z", 3, 7, 300, 20),
                compaction(),
                "{not json".into(),
                assistant("m3", "2026-10-16T10:02:00.000Z", 1, 2, 40, 0),
                synthetic(),
            ],
        );
        assert_eq!(
            root(&world, &project),
            Some(Usage {
                input: 14,
                output: 14,
                cache_read: 440,
                cache_write: 70,
                context: 41,
                peak_context: 323,
                compactions: 1,
                updated: "2026-10-16T10:02:00Z".into(),
                over_cap: vec![],
            })
        );
    }

    /// The offset is kept: earlier bytes are not read again, so a rewrite of
    /// them changes nothing and only the appended request is added.
    #[test]
    fn a_second_read_only_reads_what_was_appended() {
        let world = World::new();
        let (project, transcript) = demo(&world);
        append(
            &transcript,
            &[assistant("m1", "2026-10-16T10:00:00Z", 10, 5, 100, 50)],
        );
        assert_eq!(root(&world, &project).unwrap().input, 10);
        let len = std::fs::metadata(&transcript).unwrap().len() as usize;
        std::fs::write(&transcript, format!("{}\n", " ".repeat(len - 1))).unwrap();
        append(
            &transcript,
            &[assistant("m2", "2026-10-16T10:01:00Z", 1, 1, 1, 1)],
        );
        let usage = root(&world, &project).unwrap();
        assert_eq!((usage.input, usage.output, usage.cache_read), (11, 6, 101));
        let store = load(&project);
        let file = &store.nodes["root"].files[&*transcript.to_string_lossy()];
        assert_eq!(file.offset, std::fs::metadata(&transcript).unwrap().len());
    }

    /// A line the harness is still writing is read once it ends.
    #[test]
    fn a_line_still_being_written_waits_for_its_end() {
        let world = World::new();
        let (project, transcript) = demo(&world);
        let line = assistant("m1", "2026-10-16T10:00:00Z", 10, 5, 100, 50);
        let (head, tail) = line.split_at(30);
        std::fs::write(&transcript, head).unwrap();
        assert_eq!(root(&world, &project), None);
        std::fs::write(&transcript, format!("{head}{tail}\n")).unwrap();
        assert_eq!(root(&world, &project).unwrap().input, 10);
    }

    /// Nothing understood, or no transcript at all, is `null`; a transcript
    /// the harness deleted keeps what was read from it.
    #[test]
    fn nothing_understood_is_none_and_a_deleted_transcript_keeps_its_counts() {
        let world = World::new();
        let (project, transcript) = demo(&world);
        assert_eq!(root(&world, &project), None);
        append(&transcript, &[user(), "{not json".into(), "[]".into()]);
        assert_eq!(root(&world, &project), None);
        append(
            &transcript,
            &[assistant("m1", "2026-10-16T10:00:00Z", 10, 5, 100, 50)],
        );
        let read = root(&world, &project).unwrap();
        std::fs::remove_file(&transcript).unwrap();
        assert_eq!(root(&world, &project), Some(read));
    }

    /// Claude Code keeps a session's subagents next to it: their tokens are
    /// the node's, their context is not.
    #[test]
    fn subagent_tokens_count_toward_the_node_but_not_its_context() {
        let world = World::new();
        let (project, transcript) = demo(&world);
        append(
            &transcript,
            &[assistant("m1", "2026-10-16T10:00:00Z", 10, 5, 100, 50)],
        );
        let subagents = transcript.with_extension("").join("subagents");
        std::fs::create_dir_all(&subagents).unwrap();
        append(
            &subagents.join("agent-a1.jsonl"),
            &[
                assistant("s1", "2026-10-16T10:05:00Z", 1000, 1, 9000, 0),
                compaction(),
            ],
        );
        let usage = root(&world, &project).unwrap();
        assert_eq!(
            (usage.input, usage.output, usage.cache_read),
            (1010, 6, 9100)
        );
        assert_eq!(
            (usage.context, usage.peak_context, usage.compactions),
            (160, 160, 0)
        );
    }

    /// Codex writes cumulative totals: each growth is one request, its input
    /// split into the uncached part and the cache read, and a total that goes
    /// down (a compaction or a resume) counts that event's own use.
    #[test]
    fn codex_counts_the_growth_of_its_totals_and_a_drop_as_a_compaction() {
        let world = World::new();
        let project = world.project("demo", "a.sock");
        project
            .update_coordinator(|c| {
                c.agent = CODEX.into();
                c.sessions = vec!["0199a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b".into()];
            })
            .unwrap();
        let dir = world.home.path().join(".codex/sessions/2026/10/16");
        std::fs::create_dir_all(&dir).unwrap();
        append(
            &dir.join("rollout-2026-10-16T10-00-00-0199a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b.jsonl"),
            &[
                json!({"type": "session_meta", "payload": {"id": "x"}}).to_string(),
                token_count("2026-10-16T10:00:00Z", [1000, 800, 50], [1000, 800, 50]),
                token_count("2026-10-16T10:00:01Z", [1000, 800, 50], [1000, 800, 50]),
                token_count("2026-10-16T10:01:00Z", [1500, 1200, 80], [500, 400, 30]),
                token_count("2026-10-16T10:02:00Z", [300, 100, 10], [300, 100, 10]),
            ],
        );
        assert_eq!(
            root(&world, &project),
            Some(Usage {
                input: 500,
                output: 90,
                cache_read: 1300,
                cache_write: 0,
                context: 300,
                peak_context: 1000,
                compactions: 1,
                updated: "2026-10-16T10:02:00Z".into(),
                over_cap: vec![],
            })
        );
    }

    /// The ticker keeps every session id Herdr reported for a node's pane,
    /// once each, and a node whose pane changed session sums both transcripts.
    #[test]
    fn a_pane_that_changed_session_sums_both_transcripts() {
        let world = World::new();
        let project = world.project("demo", "a.sock");
        let worktree = world.home.path().join("wt");
        std::fs::create_dir(&worktree).unwrap();
        let t = world.thread(&project, &worktree, |_| {});
        let cwd = project.canonical_dir().to_string_lossy().into_owned();
        let agents = |root: &str, thread: &str| {
            format!(
                r#"[{{"pane_id":"w1:p1","tab_id":"w1:t1","workspace_id":"w1","name":"hp-demo-coordinator","agent":"claude","agent_status":"idle","cwd":"{cwd}","agent_session":{{"value":"{root}"}}}},{{"pane_id":"w2:p1","tab_id":"w2:t1","workspace_id":"w2","name":"{}","agent":"claude","agent_status":"idle","cwd":"{}","agent_session":{{"kind":"id","value":"{thread}"}}}}]"#,
                t.agent_name, t.cwd
            )
        };
        *world.panes.borrow_mut() = format!(
            "[{},{}]",
            world.coordinator_pane(&project),
            crate::scenarios::pane_json("w2", "w2:t1", "w2:p1", &t.cwd)
        );
        let mut memory = crate::steps::Memory::new(&world.ctx());
        *world.agents.borrow_mut() = agents("root-1", "t-a");
        crate::ticker::tick_for_test(&world.ctx(), &mut memory);
        *world.agents.borrow_mut() = agents("root-1", "t-b");
        crate::ticker::tick_for_test(&world.ctx(), &mut memory);
        assert_eq!(
            crate::thread::load(&project, &t.id).unwrap().sessions,
            vec!["t-a".to_string(), "t-b".to_string()]
        );
        assert_eq!(
            project.coordinator().unwrap().sessions,
            vec!["root-1".to_string()]
        );

        append(
            &claude_transcript(&world, &t.cwd, "t-a"),
            &[assistant("a1", "2026-10-16T10:00:00Z", 10, 5, 100, 50)],
        );
        append(
            &claude_transcript(&world, &t.cwd, "t-b"),
            &[assistant("b1", "2026-10-16T11:00:00Z", 1, 2, 30, 0)],
        );
        let usage = refresh(&world.ctx(), &project)
            .unwrap()
            .usage(&t.id)
            .unwrap();
        assert_eq!((usage.input, usage.output, usage.cache_read), (11, 7, 130));
        assert_eq!((usage.context, usage.peak_context), (31, 160));
    }

    fn caps(world: &World, table: &str) {
        let dir = world.ctx().config_dir;
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("config.toml"), table).unwrap();
    }

    fn toasts(world: &World) -> Vec<String> {
        world
            .runner
            .calls
            .borrow()
            .iter()
            .map(|c| c.display())
            .filter(|c| c.contains("notification show"))
            .collect()
    }

    /// A cap notifies once when a node reaches it, without sound, and again
    /// only after the node went below it and reached it once more.
    #[test]
    fn a_cap_notifies_once_per_crossing() {
        let world = World::new();
        world
            .runner
            .on("notification show", ok(r#"{"result":{"shown":true}}"#));
        let (project, transcript) = demo(&world);
        caps(&world, "[usage.default]\ncontext_warn = 1000\n");
        append(
            &transcript,
            &[assistant("m1", "2026-10-16T10:00:00Z", 10, 5, 1490, 0)],
        );
        assert_eq!(root(&world, &project).unwrap().over_cap, ["context_warn"]);
        assert_eq!(toasts(&world).len(), 1);
        assert!(
            toasts(&world)[0].contains("--sound none"),
            "{:?}",
            toasts(&world)
        );
        append(
            &transcript,
            &[assistant("m2", "2026-10-16T10:01:00Z", 10, 5, 1590, 0)],
        );
        root(&world, &project);
        assert_eq!(toasts(&world).len(), 1, "still over: no second toast");
        append(
            &transcript,
            &[
                compaction(),
                assistant("m3", "2026-10-16T10:02:00Z", 10, 5, 190, 0),
            ],
        );
        assert_eq!(
            root(&world, &project).unwrap().over_cap,
            Vec::<String>::new()
        );
        append(
            &transcript,
            &[assistant("m4", "2026-10-16T10:03:00Z", 10, 5, 1190, 0)],
        );
        root(&world, &project);
        assert_eq!(toasts(&world).len(), 2, "{:?}", toasts(&world));
        let view = crate::state::build(&world.ctx(), &[]);
        assert_eq!(view.projects[0].coordinators[0].over_cap, ["context_warn"]);
    }

    /// `tokens_warn` counts input, cache writes and output, never cache reads;
    /// a project's own table wins over the default one, and 0 turns a cap off.
    #[test]
    fn the_token_cap_leaves_cache_reads_out_and_a_project_table_wins() {
        let world = World::new();
        let (project, transcript) = demo(&world);
        let dir = project.canonical_dir().to_string_lossy().into_owned();
        caps(
            &world,
            &format!(
                "[usage.default]\ntokens_warn = 100\ncompaction_warn = 1\n\n[usage.\"{dir}\"]\ncompaction_warn = 0\n"
            ),
        );
        append(
            &transcript,
            &[
                compaction(),
                assistant("m1", "2026-10-16T10:00:00Z", 40, 9, 900_000, 50),
            ],
        );
        assert_eq!(
            root(&world, &project).unwrap().over_cap,
            Vec::<String>::new()
        );
        append(
            &transcript,
            &[assistant("m2", "2026-10-16T10:01:00Z", 1, 0, 0, 0)],
        );
        assert_eq!(root(&world, &project).unwrap().over_cap, ["tokens_warn"]);
    }

    /// Without a `[usage]` table there are no caps and nothing notifies; a
    /// record from before `sessions` loads, and one without sessions is
    /// written as before.
    #[test]
    fn without_a_usage_table_nothing_changes() {
        let world = World::new();
        world
            .runner
            .on("notification show", ok(r#"{"result":{"shown":true}}"#));
        let (project, transcript) = demo(&world);
        append(
            &transcript,
            &[assistant("m1", "2026-10-16T10:00:00Z", 10, 5, 9_000_000, 0)],
        );
        assert_eq!(
            root(&world, &project).unwrap().over_cap,
            Vec::<String>::new()
        );
        assert!(toasts(&world).is_empty());

        let worktree = world.home.path().join("wt");
        std::fs::create_dir(&worktree).unwrap();
        let t = world.thread(&project, &worktree, |_| {});
        let file = project.dir().join(format!("threads/{}.toml", t.id));
        assert!(!std::fs::read_to_string(&file).unwrap().contains("sessions"));
        assert!(
            crate::thread::load(&project, &t.id)
                .unwrap()
                .sessions
                .is_empty()
        );
        let coordinator =
            std::fs::read_to_string(project.state_dir().join("coordinator.json")).unwrap();
        let old: serde_json::Value = serde_json::from_str(&coordinator).unwrap();
        let mut old = old.as_object().unwrap().clone();
        old.remove("sessions");
        std::fs::write(
            project.state_dir().join("coordinator.json"),
            serde_json::to_string(&old).unwrap(),
        )
        .unwrap();
        assert!(project.coordinator().unwrap().sessions.is_empty());
    }

    /// The ticker reads at most every five minutes, and at once when a node
    /// stopped working.
    #[test]
    fn the_ticker_reads_every_five_minutes_or_when_a_node_stops() {
        let world = World::new();
        let (project, transcript) = demo(&world);
        let ctx = world.ctx();
        let now = jiff::Timestamp::now();
        append(
            &transcript,
            &[assistant("m1", "2026-10-16T10:00:00Z", 10, 5, 100, 50)],
        );
        tick(&ctx, &project, false, now).unwrap();
        assert_eq!(load(&project).usage("root").unwrap().input, 10);
        append(
            &transcript,
            &[assistant("m2", "2026-10-16T10:01:00Z", 1, 0, 0, 0)],
        );
        tick(&ctx, &project, false, now).unwrap();
        assert_eq!(
            load(&project).usage("root").unwrap().input,
            10,
            "not due yet"
        );
        tick(&ctx, &project, true, now).unwrap();
        assert_eq!(
            load(&project).usage("root").unwrap().input,
            11,
            "a node stopped"
        );
        append(
            &transcript,
            &[assistant("m3", "2026-10-16T10:02:00Z", 1, 0, 0, 0)],
        );
        let later = now + jiff::SignedDuration::from_secs(REFRESH_SECS + 1);
        tick(&ctx, &project, false, later).unwrap();
        assert_eq!(load(&project).usage("root").unwrap().input, 12, "due again");
    }
}

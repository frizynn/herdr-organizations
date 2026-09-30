//! Token usage per node, read from the session logs the agent CLIs already
//! write. Herdr reports which session runs in each pane; the ticker records
//! that session under its node and reads only the bytes appended since the
//! last tick. No agent spends a turn on any of this.

use std::collections::BTreeMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::herdr::Agent;
use crate::organizations::{self, ROOT_ID};
use crate::paths::{Ctx, Env};
use crate::project::{self, Project};
use crate::{coordinator, thread};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Tokens {
    /// Input that was not served from cache and did not write it.
    pub input: u64,
    pub cache_write: u64,
    pub cache_read: u64,
    pub output: u64,
    /// Model requests.
    pub calls: u64,
}

impl Tokens {
    pub fn total(&self) -> u64 {
        self.input + self.cache_write + self.cache_read + self.output
    }

    fn add(&mut self, other: &Tokens) {
        self.input += other.input;
        self.cache_write += other.cache_write;
        self.cache_read += other.cache_read;
        self.output += other.output;
        self.calls += other.calls;
    }
}

/// Where reading stopped in one log file, and what it has contributed.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
struct Cursor {
    offset: u64,
    tokens: Tokens,
    /// Claude repeats a request's usage on every content block it logs; the
    /// request already counted is remembered so later blocks only raise it.
    last_request: String,
    last_request_tokens: Tokens,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Session {
    pub harness: String,
    pub model: String,
    files: BTreeMap<String, Cursor>,
}

impl Session {
    pub fn tokens(&self) -> Tokens {
        let mut sum = Tokens::default();
        for cursor in self.files.values() {
            sum.add(&cursor.tokens);
        }
        sum
    }
}

/// Node id (`root` for the project coordinator) -> session id -> usage. A
/// restarted node keeps its earlier sessions, so its total survives restarts.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    pub nodes: BTreeMap<String, BTreeMap<String, Session>>,
    pub updated: String,
    /// Codex's dated session folders before this one were already searched.
    codex_searched_to: String,
}

fn state_path(project: &Project) -> PathBuf {
    project.state_dir().join("usage.json")
}

pub fn load(project: &Project) -> State {
    project::read_json(&state_path(project)).unwrap_or_default()
}

/// The ticker's step: refresh, and write only when something was used. Only
/// the ticker writes this file; the write happens under the project lock.
pub fn record(env: &Env, project: &Project, agents: &[Agent]) -> Result<()> {
    let before = load(project);
    let mut state = before.clone();
    refresh(env, project, agents, &mut state);
    if state == before {
        return Ok(());
    }
    state.updated = project::now();
    let _lock = project.lock()?;
    project::write_json(&state_path(project), &state)
}

/// Finds the sessions of every local node and reads what their logs have
/// gained. A session is attributed twice over: by the pane herdr reports it
/// in, and by a working folder only that node uses, which also finds sessions
/// that ran while no ticker was watching. Remote nodes are skipped: their logs
/// are on the other machine.
pub fn refresh(env: &Env, project: &Project, agents: &[Agent], state: &mut State) {
    let nodes: Vec<thread::Thread> = thread::list(project)
        .into_iter()
        .filter(|t| !t.is_remote())
        .collect();
    let mut folders: BTreeMap<String, Vec<&str>> = BTreeMap::new();
    if let Some(record) = project.coordinator() {
        let agent = agents
            .iter()
            .find(|a| coordinator::agent_matches(&record, a));
        note_agent(state, ROOT_ID, agent);
        folders.entry(record.cwd).or_default().push(ROOT_ID);
    }
    for node in &nodes {
        let agent = agents.iter().find(|a| thread::agent_matches(node, a));
        note_agent(state, &node.id, agent);
        folders.entry(node.cwd.clone()).or_default().push(&node.id);
    }
    let own: BTreeMap<&str, &str> = folders
        .iter()
        .filter(|(folder, nodes)| !folder.is_empty() && nodes.len() == 1)
        .map(|(folder, nodes)| (folder.as_str(), nodes[0]))
        .collect();
    for (folder, node) in &own {
        for id in claude_sessions_in(env, folder) {
            note_session(state, node, "claude", &id, None);
        }
    }
    find_codex_sessions(env, state, &own);
    for sessions in state.nodes.values_mut() {
        for (id, session) in sessions.iter_mut() {
            read_session(env, id, session);
        }
    }
}

fn note_agent(state: &mut State, node: &str, agent: Option<&Agent>) {
    if let Some(agent) = agent {
        note_session(state, node, &agent.agent, &agent.agent_session.value, None);
    }
}

fn note_session(state: &mut State, node: &str, harness: &str, id: &str, log: Option<&Path>) {
    // The id becomes part of a file name to look for, so only plain ids count.
    let plain = !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    if !plain || !matches!(harness, "claude" | "codex") {
        return;
    }
    let session = state
        .nodes
        .entry(node.to_string())
        .or_default()
        .entry(id.to_string())
        .or_default();
    session.harness = harness.to_string();
    if let Some(log) = log {
        session
            .files
            .entry(log.to_string_lossy().into_owned())
            .or_default();
    }
}

/// Claude keeps a folder's sessions under that folder's path with every
/// character that is not a letter or digit turned into a dash.
fn claude_sessions_in(env: &Env, folder: &str) -> Vec<String> {
    let name: String = folder
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let dir = home_dir(env, "CLAUDE_CONFIG_DIR", ".claude")
        .join("projects")
        .join(name);
    let mut logs = Vec::new();
    collect_logs(&dir, 0, &mut logs);
    logs.iter()
        .filter_map(|log| Some(log.file_stem()?.to_str()?.to_string()))
        .collect()
}

/// Codex writes a session's working folder on the first line of its log. The
/// dated folders are searched once; only the last two are looked at again,
/// which is where a session that started since the last tick can be.
fn find_codex_sessions(env: &Env, state: &mut State, own: &BTreeMap<&str, &str>) {
    let sessions = home_dir(env, "CODEX_HOME", ".codex").join("sessions");
    let mut days: Vec<PathBuf> = subdirs(&sessions)
        .iter()
        .flat_map(|year| subdirs(year))
        .flat_map(|month| subdirs(&month))
        .collect();
    days.sort();
    let day_name = |day: &Path| {
        day.strip_prefix(&sessions)
            .map(|d| d.to_string_lossy().into_owned())
            .unwrap_or_default()
    };
    let searched_to = state.codex_searched_to.clone();
    for day in days.iter().filter(|day| day_name(day) >= searched_to) {
        let mut logs = Vec::new();
        collect_logs(day, 0, &mut logs);
        for log in logs {
            let Some(meta) = first_line(&log) else {
                continue;
            };
            let payload = &meta["payload"];
            let folder = payload["cwd"].as_str().unwrap_or_default();
            if let (Some(node), Some(id)) = (own.get(folder), payload["id"].as_str()) {
                note_session(state, node, "codex", id, Some(&log));
            }
        }
    }
    if let Some(day) = days.iter().rev().nth(1).or(days.first()) {
        state.codex_searched_to = day_name(day);
    }
}

fn first_line(path: &Path) -> Option<serde_json::Value> {
    use std::io::BufRead;
    let mut line = String::new();
    std::io::BufReader::new(std::fs::File::open(path).ok()?)
        .read_line(&mut line)
        .ok()?;
    serde_json::from_str(&line).ok()
}

fn read_session(env: &Env, id: &str, session: &mut Session) {
    let files = match session.harness.as_str() {
        "claude" => claude_files(env, id),
        "codex" => codex_files(env, id, session.files.keys().next()),
        _ => return,
    };
    for file in files {
        let key = file.to_string_lossy().into_owned();
        let mut cursor = session.files.remove(&key).unwrap_or_default();
        while let Some(text) = appended(&file, &mut cursor.offset) {
            let model = match session.harness.as_str() {
                "claude" => read_claude(&text, &mut cursor),
                _ => read_codex(&text, &mut cursor),
            };
            if let Some(model) = model {
                session.model = model;
            }
        }
        session.files.insert(key, cursor);
    }
}

/// A log can hold hundreds of megabytes, so it is read a piece at a time.
const READ_CHUNK: u64 = 8 * 1024 * 1024;

/// The next complete lines written after `offset`, which then moves past
/// them; `None` when there are none yet. A file that shrank was replaced, so
/// reading starts over from its beginning.
fn appended(path: &Path, offset: &mut u64) -> Option<String> {
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    if len < *offset {
        *offset = 0;
    }
    file.seek(SeekFrom::Start(*offset)).ok()?;
    let mut bytes = Vec::new();
    // One line can be longer than a chunk; keep reading until it ends.
    let complete = loop {
        let before = bytes.len();
        (&mut file).take(READ_CHUNK).read_to_end(&mut bytes).ok()?;
        if let Some(end) = bytes.iter().rposition(|b| *b == b'\n') {
            break end + 1;
        }
        if bytes.len() == before {
            return None;
        }
    };
    bytes.truncate(complete);
    *offset += complete as u64;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

fn home_dir(env: &Env, var: &str, default: &str) -> PathBuf {
    match env.var(var).filter(|v| !v.is_empty()) {
        Some(dir) => PathBuf::from(dir),
        None => env.home.join(default),
    }
}

/// The session log and the logs of the subagents it started, which Claude
/// keeps in a folder named after the session.
fn claude_files(env: &Env, id: &str) -> Vec<PathBuf> {
    let projects = home_dir(env, "CLAUDE_CONFIG_DIR", ".claude").join("projects");
    let mut files = Vec::new();
    for dir in subdirs(&projects) {
        let log = dir.join(format!("{id}.jsonl"));
        if log.is_file() {
            files.push(log);
            collect_logs(&dir.join(id), 4, &mut files);
        }
    }
    files
}

/// Codex names a log `rollout-<time>-<session id>.jsonl` under dated folders.
/// Once found the path never changes, so it is not searched for again.
fn codex_files(env: &Env, id: &str, known: Option<&String>) -> Vec<PathBuf> {
    if let Some(path) = known {
        return vec![PathBuf::from(path)];
    }
    let mut files = Vec::new();
    collect_logs(
        &home_dir(env, "CODEX_HOME", ".codex").join("sessions"),
        4,
        &mut files,
    );
    let suffix = format!("-{id}.jsonl");
    files.retain(|f| f.to_string_lossy().ends_with(&suffix));
    files
}

fn subdirs(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut dirs: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    dirs
}

/// The `.jsonl` files in `dir` and in folders up to `levels` below it.
fn collect_logs(dir: &Path, levels: usize, files: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut paths: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    paths.sort();
    for path in paths {
        if path.is_dir() && levels > 0 {
            collect_logs(&path, levels - 1, files);
        } else if path.extension().is_some_and(|e| e == "jsonl") {
            files.push(path);
        }
    }
}

fn count(value: &serde_json::Value) -> u64 {
    value.as_u64().unwrap_or(0)
}

fn read_claude(text: &str, cursor: &mut Cursor) -> Option<String> {
    let mut model = None;
    for line in text.lines().filter(|l| l.contains("\"usage\"")) {
        let Ok(row) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let message = &row["message"];
        let usage = &message["usage"];
        if row["type"] != "assistant" || !usage.is_object() {
            continue;
        }
        let seen = Tokens {
            input: count(&usage["input_tokens"]),
            cache_write: count(&usage["cache_creation_input_tokens"]),
            cache_read: count(&usage["cache_read_input_tokens"]),
            output: count(&usage["output_tokens"]),
            calls: 1,
        };
        let request = message["id"].as_str().unwrap_or_default();
        if request.is_empty() || request != cursor.last_request {
            cursor.last_request = request.to_string();
            cursor.last_request_tokens = Tokens::default();
        }
        let counted = cursor.last_request_tokens;
        let grown = Tokens {
            input: seen.input.max(counted.input),
            cache_write: seen.cache_write.max(counted.cache_write),
            cache_read: seen.cache_read.max(counted.cache_read),
            output: seen.output.max(counted.output),
            calls: 1,
        };
        cursor.tokens.add(&Tokens {
            input: grown.input - counted.input,
            cache_write: grown.cache_write - counted.cache_write,
            cache_read: grown.cache_read - counted.cache_read,
            output: grown.output - counted.output,
            calls: grown.calls - counted.calls,
        });
        cursor.last_request_tokens = grown;
        if let Some(name) = message["model"].as_str().filter(|m| !m.starts_with('<')) {
            model = Some(name.to_string());
        }
    }
    model
}

/// Codex logs a running total for the session, so the latest one replaces the
/// count instead of adding to it. Its input figure includes the cached part.
fn read_codex(text: &str, cursor: &mut Cursor) -> Option<String> {
    let mut model = None;
    for line in text
        .lines()
        .filter(|l| l.contains("\"token_count\"") || l.contains("\"turn_context\""))
    {
        let Ok(row) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let payload = &row["payload"];
        if row["type"] == "turn_context" {
            if let Some(name) = payload["model"].as_str().filter(|m| !m.is_empty()) {
                model = Some(name.to_string());
            }
            continue;
        }
        let total = &payload["info"]["total_token_usage"];
        if row["type"] != "event_msg" || payload["type"] != "token_count" || !total.is_object() {
            continue;
        }
        let cached = count(&total["cached_input_tokens"]);
        cursor.tokens = Tokens {
            input: count(&total["input_tokens"]).saturating_sub(cached),
            cache_write: count(&total["cache_write_input_tokens"]),
            cache_read: cached,
            output: count(&total["output_tokens"]),
            calls: cursor.tokens.calls + 1,
        };
    }
    model
}

/// USD per million tokens for the models whose name starts with the table key.
#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Price {
    pub input: f64,
    pub cache_write: f64,
    pub cache_read: f64,
    pub output: f64,
}

/// `[prices."<model prefix>"]` tables of `<config_dir>/config.toml`. Prices
/// change often and differ by contract, so none are built in.
pub fn load_prices(config_dir: &Path) -> BTreeMap<String, Price> {
    #[derive(Deserialize, Default)]
    struct Config {
        #[serde(default)]
        prices: BTreeMap<String, Price>,
    }
    std::fs::read_to_string(config_dir.join("config.toml"))
        .ok()
        .and_then(|text| toml::from_str::<Config>(&text).ok())
        .unwrap_or_default()
        .prices
}

fn cost(prices: &BTreeMap<String, Price>, model: &str, tokens: &Tokens) -> Option<f64> {
    let price = prices
        .iter()
        .filter(|(prefix, _)| model.starts_with(prefix.as_str()))
        .max_by_key(|(prefix, _)| prefix.len())?
        .1;
    Some(
        (tokens.input as f64 * price.input
            + tokens.cache_write as f64 * price.cache_write
            + tokens.cache_read as f64 * price.cache_read
            + tokens.output as f64 * price.output)
            / 1e6,
    )
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Row {
    pub id: String,
    pub parent: String,
    pub role: String,
    pub title: String,
    pub models: Vec<String>,
    pub tokens: Tokens,
    /// This node plus every descendant.
    pub subtree: Tokens,
    /// Absent when a session's model has no configured price.
    pub cost_usd: Option<f64>,
    pub subtree_cost_usd: Option<f64>,
    #[serde(skip)]
    prefix: String,
}

pub fn rows(
    project: &Project,
    state: &State,
    prices: &BTreeMap<String, Price>,
) -> Result<Vec<Row>> {
    let own = |id: &str| -> (Tokens, Vec<String>, Option<f64>) {
        let mut tokens = Tokens::default();
        let mut models = Vec::new();
        let mut total = (!prices.is_empty()).then_some(0.0);
        for session in state.nodes.get(id).into_iter().flat_map(|s| s.values()) {
            let used = session.tokens();
            tokens.add(&used);
            if !session.model.is_empty() && !models.contains(&session.model) {
                models.push(session.model.clone());
            }
            if used.total() > 0 {
                total = total
                    .zip(cost(prices, &session.model, &used))
                    .map(|(a, b)| a + b);
            }
        }
        (tokens, models, total)
    };
    let row = |id: &str, parent: &str, role: &str, title: &str, prefix: &str| {
        let (tokens, models, cost_usd) = own(id);
        Row {
            id: id.into(),
            parent: parent.into(),
            role: role.into(),
            title: title.into(),
            models,
            tokens,
            subtree: tokens,
            cost_usd,
            subtree_cost_usd: cost_usd,
            prefix: prefix.into(),
        }
    };
    let mut rows = vec![row(ROOT_ID, "", "coordinator", &project.slug, "")];
    for entry in organizations::tree(project)? {
        let node = &entry.thread;
        rows.push(row(
            &node.id,
            organizations::parent_id(node),
            node.role.as_str(),
            &node.title,
            &entry.prefix,
        ));
    }
    // Preorder lists a parent before its children, so walking it backwards
    // has every subtree complete before it is added to its parent.
    for index in (1..rows.len()).rev() {
        let (subtree, subtree_cost) = (rows[index].subtree, rows[index].subtree_cost_usd);
        let parent = rows[index].parent.clone();
        if let Some(target) = rows.iter_mut().find(|r| r.id == parent) {
            target.subtree.add(&subtree);
            target.subtree_cost_usd = target
                .subtree_cost_usd
                .zip(subtree_cost)
                .map(|(a, b)| a + b);
        }
    }
    Ok(rows)
}

fn short(tokens: u64) -> String {
    match tokens {
        0..1_000 => tokens.to_string(),
        1_000..1_000_000 => format!("{:.1}k", tokens as f64 / 1e3),
        1_000_000..1_000_000_000 => format!("{:.1}M", tokens as f64 / 1e6),
        _ => format!("{:.2}B", tokens as f64 / 1e9),
    }
}

fn dollars(cost: Option<f64>) -> String {
    cost.map_or("-".into(), |c| format!("${c:.2}"))
}

pub fn render(rows: &[Row]) -> String {
    let mut text =
        "node\trole\tmodel\tcalls\tinput\tcache write\tcache read\toutput\ttotal\tcost\twith children\tcost\ttitle\n"
            .to_string();
    for row in rows {
        let models = if row.models.is_empty() {
            "-".to_string()
        } else {
            row.models.join(",")
        };
        text.push_str(&format!(
            "{}{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
            row.prefix,
            row.id,
            row.role,
            models,
            row.tokens.calls,
            short(row.tokens.input),
            short(row.tokens.cache_write),
            short(row.tokens.cache_read),
            short(row.tokens.output),
            short(row.tokens.total()),
            dollars(row.cost_usd),
            short(row.subtree.total()),
            dollars(row.subtree_cost_usd),
            row.title
        ));
    }
    text
}

/// `usage <slug>`: the recorded usage, brought up to date in memory with the
/// sessions that are live now. It never writes, so it cannot race the ticker.
pub fn print(ctx: &Ctx, slug: &str, json: bool) -> Result<()> {
    let project = Project::load(&ctx.root, slug)?;
    let mut state = load(&project);
    let agents = project
        .coordinator()
        .filter(|c| Path::new(&c.socket).exists())
        .and_then(|c| {
            crate::herdr::Herdr::new(ctx.env.herdr_bin(), &c.socket, ctx.runner)
                .agent_list()
                .ok()
        })
        .unwrap_or_default();
    refresh(ctx.env, &project, &agents, &mut state);
    let rows = rows(&project, &state, &load_prices(&ctx.config_dir))?;
    if json {
        println!("{}", serde_json::to_string_pretty(&rows)?);
    } else {
        print!("{}", render(&rows));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claude_row(request: &str, input: u64, write: u64, read: u64, output: u64) -> String {
        format!(
            r#"{{"type":"assistant","message":{{"id":"{request}","model":"claude-x","usage":{{"input_tokens":{input},"cache_creation_input_tokens":{write},"cache_read_input_tokens":{read},"output_tokens":{output}}}}}}}"#
        )
    }

    fn codex_row(input: u64, cached: u64, output: u64) -> String {
        format!(
            r#"{{"type":"event_msg","payload":{{"type":"token_count","info":{{"total_token_usage":{{"input_tokens":{input},"cached_input_tokens":{cached},"output_tokens":{output}}}}}}}}}"#
        )
    }

    fn agent(kind: &str, session: &str) -> Agent {
        Agent {
            agent: kind.into(),
            agent_session: crate::herdr::AgentSession {
                value: session.into(),
            },
            ..Agent::default()
        }
    }

    #[test]
    fn a_claude_request_logged_in_several_blocks_counts_once() {
        let text = [
            claude_row("a", 2, 100, 1000, 5),
            claude_row("a", 2, 100, 1000, 40),
            claude_row("b", 3, 0, 1100, 7),
        ]
        .join("\n");
        let mut cursor = Cursor::default();
        assert_eq!(read_claude(&text, &mut cursor).as_deref(), Some("claude-x"));
        assert_eq!(
            cursor.tokens,
            Tokens {
                input: 5,
                cache_write: 100,
                cache_read: 2100,
                output: 47,
                calls: 2
            }
        );
    }

    #[test]
    fn a_request_split_across_two_reads_still_counts_once() {
        let mut cursor = Cursor::default();
        read_claude(&claude_row("a", 2, 0, 10, 5), &mut cursor);
        read_claude(&claude_row("a", 2, 0, 10, 9), &mut cursor);
        assert_eq!(cursor.tokens.output, 9);
        assert_eq!(cursor.tokens.calls, 1);
    }

    #[test]
    fn codex_totals_replace_and_split_cached_input() {
        let text = [
            r#"{"type":"turn_context","payload":{"model":"gpt-x"}}"#.to_string(),
            codex_row(100, 60, 10),
            codex_row(300, 200, 25),
        ]
        .join("\n");
        let mut cursor = Cursor::default();
        assert_eq!(read_codex(&text, &mut cursor).as_deref(), Some("gpt-x"));
        assert_eq!(
            cursor.tokens,
            Tokens {
                input: 100,
                cache_write: 0,
                cache_read: 200,
                output: 25,
                calls: 2
            }
        );
    }

    #[test]
    fn only_appended_complete_lines_are_read() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("log.jsonl");
        std::fs::write(&log, "one\ntwo\nhal").unwrap();
        let mut offset = 0;
        assert_eq!(appended(&log, &mut offset).as_deref(), Some("one\ntwo\n"));
        assert_eq!(appended(&log, &mut offset), None);
        std::fs::write(&log, "one\ntwo\nhalf\n").unwrap();
        assert_eq!(appended(&log, &mut offset).as_deref(), Some("half\n"));
        std::fs::write(&log, "new\n").unwrap();
        assert_eq!(appended(&log, &mut offset).as_deref(), Some("new\n"));
    }

    #[test]
    fn a_session_includes_its_subagents_and_is_read_incrementally() {
        let home = tempfile::tempdir().unwrap();
        let env = Env::for_test(home.path(), &[]);
        let project = home.path().join(".claude/projects/-repo");
        std::fs::create_dir_all(project.join("abc/subagents")).unwrap();
        std::fs::write(
            project.join("abc.jsonl"),
            claude_row("a", 1, 0, 10, 2) + "\n",
        )
        .unwrap();
        std::fs::write(
            project.join("abc/subagents/agent-1.jsonl"),
            claude_row("s", 4, 0, 20, 3) + "\n",
        )
        .unwrap();

        let mut state = State::default();
        note_agent(&mut state, "t-0001", Some(&agent("claude", "abc")));
        let session = state
            .nodes
            .get_mut("t-0001")
            .unwrap()
            .get_mut("abc")
            .unwrap();
        read_session(&env, "abc", session);
        assert_eq!(session.tokens().total(), 40);

        let mut log = std::fs::read_to_string(project.join("abc.jsonl")).unwrap();
        log.push_str(&(claude_row("b", 1, 0, 100, 1) + "\n"));
        std::fs::write(project.join("abc.jsonl"), log).unwrap();
        read_session(&env, "abc", session);
        assert_eq!(session.tokens().total(), 142);
        assert_eq!(session.tokens().calls, 3);
    }

    #[test]
    fn a_codex_session_is_found_by_the_id_in_its_file_name() {
        let home = tempfile::tempdir().unwrap();
        let env = Env::for_test(home.path(), &[]);
        let day = home.path().join(".codex/sessions/2026/09/30");
        std::fs::create_dir_all(&day).unwrap();
        std::fs::write(
            day.join("rollout-2026-09-30T10-00-00-abc.jsonl"),
            codex_row(50, 40, 5) + "\n",
        )
        .unwrap();
        std::fs::write(
            day.join("rollout-2026-09-30T11-00-00-xabc.jsonl"),
            codex_row(9, 9, 9) + "\n",
        )
        .unwrap();

        let mut state = State::default();
        note_agent(&mut state, "root", Some(&agent("codex", "abc")));
        let session = state.nodes.get_mut("root").unwrap().get_mut("abc").unwrap();
        read_session(&env, "abc", session);
        assert_eq!(session.tokens().total(), 55);
    }

    #[test]
    fn a_folder_used_by_one_node_attributes_its_sessions_without_a_pane() {
        let world = crate::scenarios::World::new();
        let project = world.project("demo", "sock");
        let ours = world.thread(&project, Path::new("/work/a"), |_| {});
        let second = world.thread(&project, Path::new("/work/shared"), |_| {});
        let third = world.thread(&project, Path::new("/work/shared"), |_| {});
        let home = world.home.path();
        let claude = home.join(".claude/projects/-work-a");
        std::fs::create_dir_all(&claude).unwrap();
        std::fs::write(claude.join("c1.jsonl"), claude_row("a", 1, 0, 10, 2) + "\n").unwrap();
        let day = home.join(".codex/sessions/2026/09/30");
        std::fs::create_dir_all(&day).unwrap();
        let meta = |id: &str, cwd: &str| {
            format!(r#"{{"type":"session_meta","payload":{{"id":"{id}","cwd":"{cwd}"}}}}"#) + "\n"
        };
        std::fs::write(
            day.join("rollout-x-k1.jsonl"),
            meta("k1", "/work/a") + &codex_row(50, 40, 5) + "\n",
        )
        .unwrap();
        std::fs::write(
            day.join("rollout-x-k2.jsonl"),
            meta("k2", "/work/shared") + &codex_row(9, 0, 9) + "\n",
        )
        .unwrap();

        let mut state = State::default();
        refresh(&world.env, &project, &[], &mut state);
        let total = |id: &str| -> u64 {
            state
                .nodes
                .get(id)
                .into_iter()
                .flat_map(|s| s.values())
                .map(|s| s.tokens().total())
                .sum()
        };
        assert_eq!(total(&ours.id), 13 + 55);
        assert_eq!(total(&second.id) + total(&third.id), 0);
    }

    #[test]
    fn the_ticker_step_writes_only_when_usage_changed() {
        let world = crate::scenarios::World::new();
        let project = world.project("demo", "sock");
        let node = world.thread(&project, Path::new("/work/a"), |_| {});
        record(&world.env, &project, &[]).unwrap();
        assert!(!state_path(&project).exists());

        let claude = world.home.path().join(".claude/projects/-work-a");
        std::fs::create_dir_all(&claude).unwrap();
        std::fs::write(claude.join("c1.jsonl"), claude_row("a", 1, 0, 10, 2) + "\n").unwrap();
        record(&world.env, &project, &[]).unwrap();
        let written = load(&project);
        assert_eq!(written.nodes[&node.id]["c1"].tokens().total(), 13);
        assert!(!written.updated.is_empty());

        record(&world.env, &project, &[]).unwrap();
        assert_eq!(load(&project), written);
    }

    #[test]
    fn sessions_without_a_plain_id_or_a_known_harness_are_ignored() {
        let mut state = State::default();
        note_agent(&mut state, "t-0001", Some(&agent("claude", "../etc")));
        note_agent(&mut state, "t-0001", Some(&agent("claude", "")));
        note_agent(&mut state, "t-0001", Some(&agent("gemini", "abc")));
        note_agent(&mut state, "t-0001", None);
        assert!(state.nodes.is_empty());
    }

    #[test]
    fn the_longest_matching_prefix_prices_a_model() {
        let prices = BTreeMap::from([
            (
                "claude".to_string(),
                Price {
                    input: 1.0,
                    ..Price::default()
                },
            ),
            (
                "claude-opus".to_string(),
                Price {
                    input: 10.0,
                    cache_read: 1.0,
                    ..Price::default()
                },
            ),
        ]);
        let tokens = Tokens {
            input: 1_000_000,
            cache_read: 2_000_000,
            ..Tokens::default()
        };
        assert_eq!(cost(&prices, "claude-opus-9", &tokens), Some(12.0));
        assert_eq!(cost(&prices, "claude-small", &tokens), Some(1.0));
        assert_eq!(cost(&prices, "gpt", &tokens), None);
    }
}

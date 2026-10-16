//! Token use per local node: the root coordinator, each coordinator node and
//! each thread. Herdr reports no usage, only each pane's native session id
//! (`Agent::session_id`); the ticker keeps those in the node's record
//! (`sessions`), and this module reads the harness's own transcripts for them.

use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::paths::Ctx;
use crate::project::{self, Project};

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
    pub last_key: String,
    pub cumulative: [u64; 3],
    pub requests: u64,
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub context: u64,
    pub peak_context: u64,
    pub compactions: u64,
    pub last_at: String,
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

impl Node {
    pub fn usage(&self) -> Option<Usage> {
        None
    }
}

impl Store {
    pub fn usage(&self, id: &str) -> Option<Usage> {
        self.nodes.get(id).and_then(Node::usage)
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

impl Caps {
    pub fn over(&self, _usage: &Usage) -> Vec<String> {
        Vec::new()
    }
}

pub fn path(project: &Project) -> PathBuf {
    project.state_dir().join("usage.json")
}

pub fn load(project: &Project) -> Store {
    project::read_json(&path(project)).unwrap_or_default()
}

/// Reads what every open local node's transcripts added since the last read.
pub fn refresh(_ctx: &Ctx, project: &Project) -> Result<Store> {
    Ok(load(project))
}

/// The ticker's pass.
pub fn tick(_ctx: &Ctx, _project: &Project, _stopped: bool, _now: jiff::Timestamp) -> Result<()> {
    Ok(())
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
    /// line that is not JSON is skipped.
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

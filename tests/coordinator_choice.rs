//! Choosing the coordinator's harness, model and effort: `open` and
//! `coordinator replace` take the node flags, and `profile list --json` says
//! what a project's coordinator may run. Checked against the built binary in a
//! scrubbed environment, with Herdr as a script that logs every call.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::{Value, json};

const BIN: &str = env!("CARGO_BIN_EXE_herdr-organizations");

const CONFIG: &str = r#"[profiles.deep]
agent = "codex"
model = "gpt-5.5"
effort = "high"
description = "Hard debugging"

[safety.default]
coordinator_profiles = ["claude", "deep"]
"#;

struct Fixture {
    home: tempfile::TempDir,
    root: String,
}

impl Fixture {
    /// Project `acme`, Claude Code and Codex installed and signed in, the
    /// profiles in `CONFIG`, and a Herdr that answers from `agents.json`.
    fn new() -> Fixture {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join("root").to_str().unwrap().to_string();
        let fixture = Fixture { home, root };
        let bin = fixture.bin();
        std::fs::create_dir_all(&bin).unwrap();
        let log = fixture.home().join("herdr.log");
        let agents = fixture.home().join("agents.json");
        fixture.script(
            "herdr",
            &format!(
                r#"#!/bin/sh
echo "$*" >>"{log}"
case "$*" in
  "agent list"*) printf '{{"result":{{"agents":%s}}}}\n' "$(cat "{agents}" 2>/dev/null || echo '[]')" ;;
  "pane list"*) echo '{{"result":{{"panes":[]}}}}' ;;
  "workspace create"*) echo '{{"result":{{"root_pane":{{"workspace_id":"w1","tab_id":"w1:t1","pane_id":"w1:p1"}}}}}}' ;;
  "tab create"*) echo '{{"result":{{"root_pane":{{"workspace_id":"w1","tab_id":"w1:t2","pane_id":"w1:p2"}}}}}}' ;;
  "agent start"*) echo '{{"result":{{"agent":{{"pane_id":"w1:p1","tab_id":"w1:t1","workspace_id":"w1","name":"hpc-acme","agent":"claude","agent_status":"idle"}}}}}}' ;;
  *) echo '{{"result":{{}}}}' ;;
esac
"#,
                log = log.display(),
                agents = agents.display(),
            ),
        );
        fixture.script("claude", "#!/bin/sh\n");
        fixture.script("codex", "#!/bin/sh\n");
        std::fs::write(
            fixture.home().join(".claude.json"),
            r#"{"oauthAccount":{}}"#,
        )
        .unwrap();
        std::fs::create_dir_all(fixture.home().join(".codex")).unwrap();
        std::fs::write(fixture.home().join(".codex/auth.json"), "{}").unwrap();
        std::fs::write(
            fixture.home().join(".codex/config.toml"),
            "model = \"gpt-5.4\"\n",
        )
        .unwrap();
        let config = fixture.home().join(".config/herdr-projects");
        std::fs::create_dir_all(&config).unwrap();
        std::fs::write(config.join("config.toml"), CONFIG).unwrap();
        std::fs::write(fixture.socket(), "").unwrap();
        assert!(fixture.run(&["new", "acme"]).status.success());
        fixture
    }

    fn home(&self) -> &Path {
        self.home.path()
    }

    fn bin(&self) -> PathBuf {
        self.home().join("bin")
    }

    fn socket(&self) -> PathBuf {
        self.home().join("herdr.sock")
    }

    fn script(&self, name: &str, text: &str) {
        use std::os::unix::fs::PermissionsExt as _;
        let path = self.bin().join(name);
        std::fs::write(&path, text).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(BIN)
            .env_clear()
            .env("HOME", self.home())
            .env("PATH", format!("{}:/usr/bin:/bin", self.bin().display()))
            .env("HERDR_BIN_PATH", self.bin().join("herdr"))
            .arg("--root")
            .arg(&self.root)
            .args(args)
            .output()
            .unwrap()
    }

    fn ok(&self, args: &[&str]) -> String {
        let out = self.run(args);
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    }

    fn refused(&self, args: &[&str]) -> String {
        let out = self.run(args);
        assert!(!out.status.success(), "{args:?} was accepted");
        String::from_utf8_lossy(&out.stderr).into_owned()
    }

    fn choices(&self) -> Value {
        let text = self.ok(&["profile", "list", "--project", "acme", "--json"]);
        assert_eq!(text.lines().count(), 1, "one JSON line: {text}");
        serde_json::from_str(&text).unwrap()
    }

    fn herdr_calls(&self) -> Vec<String> {
        std::fs::read_to_string(self.home().join("herdr.log"))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn record(&self) -> Value {
        let text =
            std::fs::read_to_string(Path::new(&self.root).join("acme/.state/coordinator.json"))
                .unwrap();
        serde_json::from_str(&text).unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        // `open` starts the detached ticker; stop it with the fixture.
        let _ = self.run(&["ticker", "stop"]);
    }
}

#[test]
fn profile_list_json_says_what_a_coordinator_may_run() {
    let fixture = Fixture::new();
    assert_eq!(
        fixture.choices(),
        json!({
            "schema_version": 1,
            "project": "acme",
            "coordinator": {
                "default_profile": "claude",
                "last": null,
                "harnesses": [
                    {"harness": "claude", "profile": "claude", "default_model": "", "efforts": ["low", "medium", "high", "xhigh", "max"]},
                ],
                "profiles": [
                    {"name": "deep", "harness": "codex", "model": "gpt-5.5", "reasoning_effort": "high", "description": "Hard debugging", "builtin": false},
                    {"name": "claude", "harness": "claude", "model": "", "reasoning_effort": "", "description": "", "builtin": true},
                ],
            },
            "thread": {
                "default_profile": "claude",
                "last": null,
                "harnesses": [
                    {"harness": "claude", "profile": "claude", "default_model": "", "efforts": ["low", "medium", "high", "xhigh", "max"]},
                    {"harness": "codex", "profile": "codex", "default_model": "gpt-5.4", "efforts": ["none", "minimal", "low", "medium", "high", "xhigh", "max", "ultra"]},
                ],
                "profiles": [
                    {"name": "deep", "harness": "codex", "model": "gpt-5.5", "reasoning_effort": "high", "description": "Hard debugging", "builtin": false},
                    {"name": "claude", "harness": "claude", "model": "", "reasoning_effort": "", "description": "", "builtin": true},
                    {"name": "codex", "harness": "codex", "model": "", "reasoning_effort": "", "description": "", "builtin": true},
                ],
            },
        })
    );
    let error = fixture.refused(&["profile", "list", "--json"]);
    assert!(error.contains("--project"), "{error}");
}

#[test]
fn open_starts_the_chosen_harness_with_its_model_and_effort_and_remembers_them() {
    let fixture = Fixture::new();
    let socket = fixture.socket();
    let socket = socket.to_str().unwrap();
    fixture.ok(&[
        "open",
        "acme",
        "--socket",
        socket,
        "--tab",
        "--harness",
        "claude",
        "--model",
        "claude-opus-5-5[1m]",
        "--reasoning-effort",
        "xhigh",
    ]);
    let start = fixture
        .herdr_calls()
        .into_iter()
        .find(|call| call.starts_with("agent start"))
        .expect("the agent was started");
    assert!(start.contains("--kind claude"), "{start}");
    assert!(
        start.ends_with("-- --model claude-opus-5-5[1m] --effort xhigh"),
        "{start}"
    );
    let record = fixture.record();
    assert_eq!(
        (
            &record["profile"],
            &record["agent"],
            &record["model"],
            &record["reasoning_effort"]
        ),
        (
            &json!("claude"),
            &json!("claude"),
            &json!("claude-opus-5-5[1m]"),
            &json!("xhigh")
        )
    );
    assert_eq!(
        fixture.choices()["coordinator"]["last"],
        json!({"profile": "claude", "harness": "claude", "model": "claude-opus-5-5[1m]", "reasoning_effort": "xhigh"})
    );
}

#[test]
fn a_profile_with_its_own_model_gives_way_to_the_model_chosen_at_open() {
    let fixture = Fixture::new();
    let socket = fixture.socket();
    let socket = socket.to_str().unwrap();
    fixture.ok(&[
        "open",
        "acme",
        "--socket",
        socket,
        "--tab",
        "--profile",
        "deep",
        "--model",
        "gpt-5.6",
    ]);
    let start = fixture
        .herdr_calls()
        .into_iter()
        .find(|call| call.starts_with("agent start"))
        .unwrap();
    assert!(start.contains("--kind codex"), "{start}");
    assert!(
        start.ends_with("-- --model gpt-5.6 -c model_reasoning_effort=\"high\""),
        "{start}"
    );
    assert!(!start.contains("gpt-5.5"), "{start}");
    assert_eq!(
        fixture.choices()["coordinator"]["last"],
        json!({"profile": "deep", "harness": "codex", "model": "gpt-5.6", "reasoning_effort": "high"})
    );
}

#[test]
fn open_refuses_a_harness_off_the_list_a_bad_model_or_effort_before_any_pane() {
    let fixture = Fixture::new();
    let socket = fixture.socket();
    let socket = socket.to_str().unwrap();
    let open = |rest: &[&str]| {
        let mut args = vec!["open", "acme", "--socket", socket, "--tab"];
        args.extend_from_slice(rest);
        fixture.refused(&args)
    };
    let error = open(&["--harness", "codex"]);
    assert!(
        error.contains("profile `codex` is not allowed for the coordinator"),
        "{error}"
    );
    let error = open(&["--harness", "claude", "--model", "a;b"]);
    assert!(
        error.contains("`a;b` does not look like a model name"),
        "{error}"
    );
    let error = open(&["--harness", "claude", "--model=-x"]);
    assert!(
        error.contains("`-x` does not look like a model name"),
        "{error}"
    );
    let error = open(&["--harness", "claude", "--reasoning-effort", "ultra"]);
    assert!(
        error.contains("effort `ultra` is not one of low, medium, high, xhigh, max for claude"),
        "{error}"
    );
    let error = open(&["--profile", "claude", "--harness", "claude"]);
    assert!(error.contains("cannot be used with"), "{error}");
    let created: Vec<String> = fixture
        .herdr_calls()
        .into_iter()
        .filter(|call| call.contains("create") || call.starts_with("agent start"))
        .collect();
    assert!(created.is_empty(), "{created:#?}");
    assert!(
        !Path::new(&fixture.root)
            .join("acme/.state/coordinator.json")
            .exists()
    );
}

#[test]
fn replace_starts_the_new_coordinator_on_the_chosen_harness_model_and_effort() {
    let fixture = Fixture::new();
    let socket = fixture.socket();
    let dir = std::fs::canonicalize(Path::new(&fixture.root).join("acme")).unwrap();
    std::fs::create_dir_all(Path::new(&fixture.root).join("acme/.state")).unwrap();
    std::fs::write(
        Path::new(&fixture.root).join("acme/.state/coordinator.json"),
        json!({"socket": socket, "workspace_id": "w1", "tab_id": "w1:t1", "pane_id": "w1:p1", "agent": "codex", "profile": "codex"}).to_string(),
    )
    .unwrap();
    std::fs::write(
        fixture.home().join("agents.json"),
        json!([{"pane_id": "w1:p1", "tab_id": "w1:t1", "workspace_id": "w1", "cwd": dir, "name": "hpc-acme", "agent": "codex", "agent_status": "idle"}]).to_string(),
    )
    .unwrap();
    let error = fixture.refused(&["coordinator", "replace", "acme"]);
    assert!(
        error.contains("--profile") && error.contains("--harness"),
        "{error}"
    );
    // A choice the project refuses stops nothing.
    let error = fixture.refused(&["coordinator", "replace", "acme", "--harness", "codex"]);
    assert!(
        error.contains("profile `codex` is not allowed for the coordinator"),
        "{error}"
    );
    let error = fixture.refused(&[
        "coordinator",
        "replace",
        "acme",
        "--harness",
        "claude",
        "--reasoning-effort",
        "ultra",
    ]);
    assert!(error.contains("effort `ultra` is not one of"), "{error}");
    assert!(
        !fixture
            .herdr_calls()
            .iter()
            .any(|c| c.starts_with("pane close")),
        "{:#?}",
        fixture.herdr_calls()
    );
    let message = fixture.ok(&[
        "coordinator",
        "replace",
        "acme",
        "--harness",
        "claude",
        "--model",
        "opus",
        "--reasoning-effort",
        "high",
    ]);
    assert!(message.contains("stopped 1"), "{message}");
    let calls = fixture.herdr_calls();
    let close = calls.iter().position(|c| c.starts_with("pane close w1:p1"));
    let start = calls
        .iter()
        .position(|c| c.starts_with("agent start") && c.contains("--kind claude"));
    assert!(close.is_some() && start > close, "{calls:#?}");
    assert!(
        calls[start.unwrap()].ends_with("-- --model opus --effort high"),
        "{}",
        calls[start.unwrap()]
    );
    assert_eq!(
        fixture.choices()["coordinator"]["last"],
        json!({"profile": "claude", "harness": "claude", "model": "opus", "reasoning_effort": "high"})
    );
}

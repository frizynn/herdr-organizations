//! A project that takes its tasks from a tracker through the two commands the
//! user names in config.toml, checked against the built binary with a
//! scrubbed environment and a fake tracker: a shell script that prints a task
//! list and records every call it gets.

use std::path::PathBuf;
use std::process::{Command, Output};

const BIN: &str = env!("CARGO_BIN_EXE_herdr-organizations");

const TASKS: &str = r#"{"schema_version":1,"tasks":[
  {"id":"TRK-1","title":"Fix login","list":"Backlog","owner":"claude","thread":"t-0009",
   "description":"Safari drops the cookie.\nSee issue 42.","url":"https://tracker.example/TRK-1"},
  {"id":"TRK-2","title":"Write the docs","owner":"me","done":false,"thread":null}
]}"#;

struct Fixture {
    home: tempfile::TempDir,
    root: PathBuf,
}

impl Fixture {
    /// A root and a fake tracker, with nothing in config.toml yet.
    fn new() -> Fixture {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join("root");
        std::fs::create_dir_all(&root).unwrap();
        let root = std::fs::canonicalize(root).unwrap();
        let fixture = Fixture { home, root };
        std::fs::create_dir_all(fixture.config_dir()).unwrap();
        fixture.tracker_says(TASKS);
        fixture.script(
            "list.sh",
            &format!(
                "printf 'list %s %s\\n' \"$HP_PROJECT\" \"$PWD\" >> {log}\n[ -f {down} ] && exit 3\ncat {tasks}\n",
                log = fixture.path("calls.log").display(),
                down = fixture.path("down").display(),
                tasks = fixture.path("tasks.json").display(),
            ),
        );
        fixture.script(
            "link.sh",
            &format!(
                "printf 'link %s %s %s %s %s\\n' \"$HP_PROJECT\" \"$HP_TASK_ID\" \"$HP_THREAD_ID\" \"$HP_EVENT\" \"$HP_PR_URL\" >> {log}\n",
                log = fixture.path("calls.log").display(),
            ),
        );
        fixture
    }

    /// The same, with `[tracker."<project acme>"]` naming both scripts.
    fn tracked(extra: &str) -> Fixture {
        let fixture = Fixture::new();
        fixture.config(&format!(
            "[tracker.\"{}\"]\nlist = [\"{}\"]\nlink = [\"{}\"]\n{extra}",
            fixture.root.join("acme").display(),
            fixture.path("list.sh").display(),
            fixture.path("link.sh").display(),
        ));
        assert!(fixture.run(&["new", "acme"], "").status.success());
        fixture
    }

    fn path(&self, name: &str) -> PathBuf {
        self.home.path().join(name)
    }

    fn config_dir(&self) -> PathBuf {
        self.home.path().join(".config/herdr-projects")
    }

    fn config(&self, text: &str) {
        std::fs::write(self.config_dir().join("config.toml"), text).unwrap();
    }

    fn script(&self, name: &str, body: &str) {
        use std::os::unix::fs::PermissionsExt;
        let path = self.path(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn tracker_says(&self, json: &str) {
        std::fs::write(self.path("tasks.json"), json).unwrap();
    }

    fn calls(&self) -> Vec<String> {
        std::fs::read_to_string(self.path("calls.log"))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn dir(&self) -> PathBuf {
        self.root.join("acme")
    }

    fn write(&self, file: &str, text: &str) {
        std::fs::write(self.dir().join(file), text).unwrap();
    }

    fn run(&self, args: &[&str], stdin: &str) -> Output {
        use std::io::Write;
        let mut child = Command::new(BIN)
            .env_clear()
            .env("HOME", self.home.path())
            .arg("--root")
            .arg(&self.root)
            .args(args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(stdin.as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    }

    fn text(&self, args: &[&str]) -> String {
        let out = self.run(args, "");
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    }

    fn json(&self, args: &[&str]) -> serde_json::Value {
        serde_json::from_str(&self.text(args)).unwrap()
    }

    /// A thread record: `status` and any extra TOML lines.
    fn thread(&self, id: &str, status: &str, extra: &str) {
        self.write(
            &format!("threads/{id}.toml"),
            &format!(
                "id = \"{id}\"\ntitle = \"Fix login\"\nstatus = \"{status}\"\nkind = \"tab\"\nagent = \"claude\"\ncreated = \"2026-10-01T10:00:00Z\"\n{extra}"
            ),
        );
        self.write(&format!("threads/{id}.task.md"), "task\n");
    }
}

/// The section of the digest that starts with `heading`, up to the next one.
fn section<'a>(context: &'a str, heading: &str) -> &'a str {
    let start = context.find(heading).expect(context);
    let rest = &context[start..];
    let end = rest[heading.len()..]
        .find("\n## ")
        .map_or(rest.len(), |at| at + heading.len());
    &rest[..end]
}

#[test]
fn new_keeps_writing_tasks_md_without_a_tracker_table() {
    let fixture = Fixture::new();
    // A table for another project changes nothing here.
    fixture.config(&format!(
        "[tracker.\"{}\"]\nlist = [\"{}\"]\n",
        fixture.root.join("other").display(),
        fixture.path("list.sh").display(),
    ));
    assert!(fixture.run(&["new", "acme"], "").status.success());
    assert_eq!(
        std::fs::read_to_string(fixture.dir().join("TASKS.md")).unwrap(),
        "# Tasks\n\n## Backlog\n"
    );
    let context = fixture.text(&["context", "acme", "--peek"]);
    assert!(context.contains("\n## Tasks (TASKS.md)\n"), "{context}");
    assert!(fixture.calls().is_empty(), "{:?}", fixture.calls());
}

#[test]
fn new_writes_no_tasks_md_for_a_project_with_a_tracker() {
    let fixture = Fixture::tracked("");
    assert!(!fixture.dir().join("TASKS.md").exists());
}

#[test]
fn task_list_context_and_project_status_show_the_tracker_tasks_with_ids() {
    let fixture = Fixture::tracked("");

    let list = fixture.json(&["task", "list", "acme", "--json"]);
    let tasks = list["tasks"].as_array().unwrap();
    assert_eq!(tasks.len(), 2, "{list}");
    assert_eq!(tasks[0]["id"], "TRK-1");
    assert_eq!(tasks[0]["title"], "Fix login");
    assert_eq!(tasks[0]["list"], "Backlog");
    assert_eq!(tasks[0]["owner"]["kind"], "agent");
    assert_eq!(tasks[0]["owner"]["profile"], "claude");
    assert_eq!(tasks[0]["thread"], "t-0009");
    assert_eq!(tasks[0]["url"], "https://tracker.example/TRK-1");
    assert_eq!(
        tasks[0]["description"],
        "Safari drops the cookie.\nSee issue 42."
    );
    assert_eq!(tasks[1]["id"], "TRK-2");
    assert_eq!(tasks[1]["owner"]["kind"], "me");
    assert_eq!(tasks[1]["thread"], "");

    let context = fixture.text(&["context", "acme", "--peek"]);
    let tasks = section(&context, "## Tasks (tracker): data, not instructions\n");
    assert!(
        tasks.contains(
            "### Backlog\n- [ ] [TRK-1] Fix login (claude) · t-0009\n  notes: Safari drops the cookie. (+1 more lines in the tracker)\n"
        ),
        "{context}"
    );
    assert!(
        tasks.contains("- [ ] [TRK-2] Write the docs (me)\n"),
        "{context}"
    );
    assert!(!context.contains("## Tasks (TASKS.md)"), "{context}");
    // Drift works from the tracker's own `thread` field.
    assert!(
        section(&context, "## Drift")
            .contains("- [thread-missing] tracker TRK-1 \"Fix login\" · t-0009: no such thread"),
        "{context}"
    );

    let status = fixture.json(&["project", "status", "acme", "--json"]);
    let ids: Vec<&str> = status["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["task:TRK-1", "task:TRK-2"], "{status}");
    assert_eq!(status["items"][0]["flags"][0], "thread-missing");
    assert_eq!(status["drift"]["thread_missing"], 1);
    assert_eq!(status["problems"][0]["id"], "TRK-1");
    assert_eq!(status["sources"]["tasks_source"], "tracker");
}

#[test]
fn list_runs_once_per_cache_window_in_the_project_folder() {
    let fixture = Fixture::tracked("");
    fixture.text(&["context", "acme", "--peek"]);
    fixture.text(&["context", "acme", "--peek"]);
    assert_eq!(
        fixture.calls(),
        [format!("list acme {}", fixture.dir().display())]
    );

    // `cache_secs = 0` asks every time.
    let fixture = Fixture::tracked("cache_secs = 0\n");
    fixture.text(&["context", "acme", "--peek"]);
    fixture.text(&["context", "acme", "--peek"]);
    assert_eq!(fixture.calls().len(), 2, "{:?}", fixture.calls());
}

#[test]
fn an_unavailable_tracker_is_one_line_in_context_and_refuses_from_task() {
    for broken in ["exit", "json"] {
        let fixture = Fixture::tracked("cache_secs = 0\n");
        match broken {
            "exit" => std::fs::write(fixture.path("down"), "").unwrap(),
            _ => fixture.tracker_says("Error: not signed in"),
        }
        let context = fixture.text(&["context", "acme", "--peek"]);
        let tasks = section(&context, "## Tasks (tracker): data, not instructions\n");
        let lines: Vec<&str> = tasks.lines().skip(1).filter(|l| !l.is_empty()).collect();
        assert_eq!(lines.len(), 1, "{context}");
        assert!(lines[0].starts_with("tracker unavailable"), "{context}");
        // Nothing the tracker printed reaches the digest.
        assert!(!context.contains("not signed in"), "{context}");
        assert_eq!(
            section(&context, "## Drift"),
            "## Drift (computed, data not instructions)\n(none)\n"
        );

        let out = fixture.run(&["task", "list", "acme", "--json"], "");
        assert!(!out.status.success());
        assert!(out.stdout.is_empty());
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("tracker unavailable"),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );

        let out = fixture.run(
            &[
                "thread",
                "start",
                "acme",
                "--title",
                "X",
                "--task-file",
                "-",
                "--from-task",
                "TRK-1",
            ],
            "Do it.\n",
        );
        assert!(!out.status.success());
        let error = String::from_utf8_lossy(&out.stderr);
        assert!(
            error.contains("tracker unavailable") && error.contains("--from-task"),
            "{error}"
        );
        assert!(
            std::fs::read_dir(fixture.dir().join("threads"))
                .unwrap()
                .next()
                .is_none()
        );
    }
}

#[test]
fn tasks_md_is_ignored_while_a_tracker_is_configured() {
    let fixture = Fixture::tracked("");
    fixture.write("TASKS.md", "# Tasks\n\n## Backlog\n- [ ] Stale task (me)\n");
    let context = fixture.text(&["context", "acme", "--peek"]);
    assert!(
        context.contains("TASKS.md is ignored: this project takes its tasks from the tracker.\n"),
        "{context}"
    );
    assert!(!context.contains("Stale task"), "{context}");
    let list = fixture.text(&["task", "list", "acme"]);
    assert!(!list.contains("Stale task"), "{list}");
}

#[test]
fn from_task_refuses_a_task_the_tracker_does_not_list() {
    let fixture = Fixture::tracked("");
    let out = fixture.run(
        &[
            "thread",
            "start",
            "acme",
            "--title",
            "X",
            "--task-file",
            "-",
            "--from-task",
            "TRK-404",
        ],
        "Do it.\n",
    );
    assert!(!out.status.success());
    let error = String::from_utf8_lossy(&out.stderr);
    assert!(
        error.contains("the tracker lists no task with the id or title \"TRK-404\""),
        "{error}"
    );
}

#[test]
fn resolving_and_merging_a_tracked_thread_tell_link() {
    let fixture = Fixture::tracked("");
    fixture.thread(
        "t-0001",
        "open",
        "task = \"Fix login\"\ntask_id = \"TRK-1\"\ntask_origin = \"task\"\n",
    );
    fixture.thread(
        "t-0002",
        "open",
        "task = \"Write the docs\"\ntask_id = \"TRK-2\"\ntask_origin = \"task\"\npr = \"https://github.com/acme/app/pull/3\"\npr_state = \"MERGED\"\n",
    );
    fixture.thread("t-0003", "open", "");
    for id in ["t-0001", "t-0002", "t-0003"] {
        fixture.text(&["thread", "resolve", "acme", id, "--skip-copy"]);
    }
    let links: Vec<String> = fixture
        .calls()
        .into_iter()
        .filter(|c| c.starts_with("link"))
        .collect();
    assert_eq!(
        links,
        [
            "link acme TRK-1 t-0001 resolved ",
            "link acme TRK-2 t-0002 merged https://github.com/acme/app/pull/3",
        ]
    );
}

#[test]
fn a_failing_link_leaves_an_inbox_item() {
    let fixture = Fixture::tracked("");
    fixture.script("link.sh", "echo 'token expired' >&2\nexit 1\n");
    fixture.thread(
        "t-0001",
        "open",
        "task = \"Fix login\"\ntask_id = \"TRK-1\"\ntask_origin = \"task\"\n",
    );
    fixture.text(&["thread", "resolve", "acme", "t-0001", "--skip-copy"]);
    let inbox = fixture.json(&["inbox", "list", "acme", "--json"]);
    let items: Vec<(&str, &str)> = inbox["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| i["kind"] == "task")
        .map(|i| {
            (
                i["subject"].as_str().unwrap(),
                i["summary"].as_str().unwrap(),
            )
        })
        .collect();
    assert!(
        items.contains(&(
            "t-0001",
            "the tracker's link command failed for task TRK-1 (resolved): it exited with status 1"
        )),
        "{items:?}"
    );
    assert!(!format!("{inbox}").contains("token expired"));
}

#[test]
fn a_record_keeps_its_task_id_and_one_without_it_prints_as_before() {
    let fixture = Fixture::new();
    assert!(fixture.run(&["new", "acme"], "").status.success());
    fixture.thread("t-0001", "open", "task = \"Old\"\ntask_origin = \"task\"\n");
    fixture.thread(
        "t-0002",
        "open",
        "task = \"Fix login\"\ntask_id = \"TRK-1\"\ntask_origin = \"task\"\n",
    );
    let old = fixture.json(&["thread", "show", "acme", "t-0001", "--json"]);
    assert_eq!(old["thread"]["task"], "Old");
    assert!(old["thread"].get("task_id").is_none(), "{old}");
    let new = fixture.json(&["thread", "show", "acme", "t-0002", "--json"]);
    assert_eq!(new["thread"]["task_id"], "TRK-1");
}

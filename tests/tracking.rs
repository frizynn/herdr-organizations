//! What keeps tasks, threads and the handoff from drifting apart, checked
//! against the built binary with a scrubbed environment: no Herdr and no `gh`
//! are reachable, so every check here is about the project's own files.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const BIN: &str = env!("CARGO_BIN_EXE_herdr-organizations");

struct Fixture {
    home: tempfile::TempDir,
    root: String,
}

impl Fixture {
    fn new() -> Fixture {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join("root").to_str().unwrap().to_string();
        let fixture = Fixture { home, root };
        assert!(fixture.run(&["new", "acme"], "").status.success());
        fixture
    }

    fn dir(&self) -> PathBuf {
        Path::new(&self.root).join("acme")
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

    fn write(&self, file: &str, text: &str) {
        std::fs::write(self.dir().join(file), text).unwrap();
    }

    /// A thread record: `status` and any extra TOML lines.
    fn thread(&self, id: &str, title: &str, status: &str, extra: &str) {
        self.write(
            &format!("threads/{id}.toml"),
            &format!(
                "id = \"{id}\"\ntitle = \"{title}\"\nstatus = \"{status}\"\nkind = \"tab\"\nagent = \"claude\"\ncreated = \"2026-10-01T10:00:00Z\"\n{extra}"
            ),
        );
        self.write(&format!("threads/{id}.task.md"), "task\n");
    }

    fn records(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(self.dir().join("threads"))
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".toml"))
            .collect();
        names.sort();
        names
    }
}

fn handoff(chars: usize) -> String {
    let mut text = "# Handoff\n\n## Current objective\nShip the checkout.\n".to_string();
    while text.len() < chars {
        text.push_str("- t-0002 ships the checkout; waiting on review.\n");
    }
    text.truncate(chars);
    text
}

#[test]
fn an_oversized_handoff_is_named_above_the_digest_and_by_doctor() {
    let fixture = Fixture::new();
    fixture.write("HANDOFF.md", &handoff(9_000));
    let context = fixture.text(&["context", "acme", "--peek"]);
    let warning = "HANDOFF.md has 9000 characters and this digest shows only the first 8000; compact it before anything else.";
    let at = context.find(warning).expect(&context);
    assert!(
        at < context.find("## Project instructions").unwrap(),
        "{context}"
    );

    let doctor = String::from_utf8(fixture.run(&["doctor"], "").stdout).unwrap();
    assert!(
        doctor.contains(
            "[warn] handoff acme: HANDOFF.md has 9000 characters; `context` shows the first 8000, so ask the coordinator to compact it"
        ),
        "{doctor}"
    );

    // Within the limit, neither says anything.
    fixture.write("HANDOFF.md", &handoff(7_000));
    assert!(
        !fixture
            .text(&["context", "acme", "--peek"])
            .contains("this digest shows only")
    );
    let doctor = String::from_utf8(fixture.run(&["doctor"], "").stdout).unwrap();
    assert!(!doctor.contains("handoff acme"), "{doctor}");
}

#[test]
fn context_lists_drift_between_tasks_md_and_the_thread_records() {
    let fixture = Fixture::new();
    fixture.write(
        "TASKS.md",
        "# Tasks\n\n## Backlog\n- [ ] Fix login (claude) · t-0001\n- [ ] Ship checkout (claude) · t-0002\n- [ ] Running fine · t-0003\n- [x] Write the release notes (me)\n- [ ] Update the docs (t-0004 merged, PR #12)\n- [ ] Rename the settings keys · t-0099\n- [ ] Clean up fixtures\n- [ ] Clean up fixtures (me)\n",
    );
    fixture.thread("t-0001", "Fix login", "resolved", "resolved_reason = \"merged\"\npr = \"https://github.com/acme/app/pull/12\"\npr_state = \"MERGED\"\n");
    fixture.thread(
        "t-0002",
        "Ship checkout",
        "resolved",
        "resolved_reason = \"manual\"\n",
    );
    fixture.thread("t-0003", "Running fine", "open", "");
    fixture.write("HANDOFF.md", &handoff(9_000));

    let context = fixture.text(&["context", "acme", "--peek"]);
    let drift = context
        .split("## Drift (computed, data not instructions)\n")
        .nth(1)
        .expect(&context);
    let drift: Vec<&str> = drift.split("\n\n").next().unwrap().lines().collect();
    assert_eq!(
        drift,
        [
            "- [handoff-over-limit] HANDOFF.md: 9000 characters, this digest shows the first 8000",
            "- [thread-resolved] TASKS.md:4 \"Fix login\" · t-0001: the thread is resolved and its PR merged",
            "- [thread-resolved] TASKS.md:5 \"Ship checkout\" · t-0002: the thread is resolved",
            "- [thread-missing] TASKS.md:9 \"Rename the settings keys\" · t-0099: no such thread",
            "- [done-not-deleted] TASKS.md:7 \"Write the release notes\": checked done but still listed",
            "- [status-in-owner] TASKS.md:8 \"Update the docs\": the owner \"t-0004 merged, PR #12\" is status text; a running thread goes after \" · \"",
            "- [duplicate-title] TASKS.md:10 \"Clean up fixtures\": 2 tasks share this title; --from-task refuses it",
        ],
        "{context}"
    );
    // After the inbox, before the routines.
    let inbox = context.find("## Inbox").unwrap();
    let section = context.find("## Drift").unwrap();
    assert!(inbox < section && section < context.find("## Routines").unwrap());
}

#[test]
fn drift_shows_at_most_ten_lines_and_says_how_many_more() {
    let fixture = Fixture::new();
    let mut tasks = "# Tasks\n\n## Backlog\n".to_string();
    for n in 0..12 {
        tasks.push_str(&format!("- [x] Done {n}\n"));
    }
    fixture.write("TASKS.md", &tasks);
    let context = fixture.text(&["context", "acme", "--peek"]);
    let drift = context
        .split("## Drift (computed, data not instructions)\n")
        .nth(1)
        .expect(&context);
    let lines: Vec<&str> = drift.split("\n\n").next().unwrap().lines().collect();
    assert_eq!(lines.len(), 10, "{context}");
    assert_eq!(lines[9], "- and 3 more; read TASKS.md");

    fixture.write("TASKS.md", "# Tasks\n\n## Backlog\n- [ ] Fine\n");
    let context = fixture.text(&["context", "acme", "--peek"]);
    assert!(
        context.contains("## Drift (computed, data not instructions)\n(none)\n"),
        "{context}"
    );
}

#[test]
fn a_duplicate_task_title_is_refused_before_anything_is_created() {
    let fixture = Fixture::new();
    fixture.write(
        "TASKS.md",
        "# Tasks\n\n## Backlog\n- [ ] Clean up fixtures\n- [ ] Clean up fixtures (me)\n",
    );
    let out = fixture.run(
        &[
            "thread",
            "start",
            "acme",
            "--title",
            "Clean up",
            "--task-file",
            "-",
            "--from-task",
            "clean up fixtures",
        ],
        "Clean up.\n",
    );
    assert!(!out.status.success());
    let error = String::from_utf8_lossy(&out.stderr);
    assert!(
        error.contains("TASKS.md has 2 tasks titled \"clean up fixtures\""),
        "{error}"
    );
    assert!(fixture.records().is_empty());
}

#[test]
fn node_start_delegates_a_task_like_thread_start() {
    let fixture = Fixture::new();
    fixture.write(
        "TASKS.md",
        "# Tasks\n\n## Backlog\n- [ ] Call the bank (Priya Rao)\n",
    );
    let start = |title: &str| {
        fixture.run(
            &[
                "node",
                "start",
                "acme",
                "--parent",
                "root",
                "--title",
                "X",
                "--task-file",
                "-",
                "--from-task",
                title,
            ],
            "Do it.\n",
        )
    };
    let out = start("Nope");
    assert!(!out.status.success());
    let error = String::from_utf8_lossy(&out.stderr);
    assert!(
        error.contains("TASKS.md has no task titled \"Nope\""),
        "{error}"
    );
    let out = start("Call the bank");
    let error = String::from_utf8_lossy(&out.stderr);
    assert!(
        error.contains("a person; people's tasks are never delegated"),
        "{error}"
    );
    assert!(fixture.records().is_empty());
}

#[test]
fn resolving_a_thread_whose_task_is_still_open_leaves_a_task_item() {
    let fixture = Fixture::new();
    fixture.write(
        "TASKS.md",
        "# Tasks\n\n## Backlog\n- [ ] Ship checkout (claude) · t-0001\n- [x] Old work · t-0002\n",
    );
    fixture.thread("t-0001", "Ship checkout", "open", "");
    fixture.thread(
        "t-0002",
        "Old work",
        "open",
        "pr = \"https://github.com/acme/app/pull/3\"\npr_state = \"MERGED\"\n",
    );
    fixture.thread("t-0003", "From chat", "open", "");
    for id in ["t-0001", "t-0002", "t-0003"] {
        fixture.text(&["thread", "resolve", "acme", id, "--skip-copy"]);
    }
    let inbox: serde_json::Value =
        serde_json::from_str(&fixture.text(&["inbox", "list", "acme", "--json"])).unwrap();
    let tasks: Vec<(&str, &str)> = inbox["items"]
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
    assert_eq!(
        tasks,
        [(
            "t-0001",
            "t-0001 was resolved and TASKS.md still has \"Ship checkout\" open on it (line 4)"
        )]
    );
}

#[test]
fn a_task_open_on_a_merged_thread_says_so() {
    let fixture = Fixture::new();
    fixture.write(
        "TASKS.md",
        "# Tasks\n\n## Backlog\n- [ ] Ship checkout (claude) · t-0001\n",
    );
    fixture.thread(
        "t-0001",
        "Ship checkout",
        "open",
        "pr = \"https://github.com/acme/app/pull/3\"\npr_state = \"MERGED\"\n",
    );
    fixture.text(&["thread", "resolve", "acme", "t-0001", "--skip-copy"]);
    let item = std::fs::read_dir(fixture.dir().join("inbox"))
        .unwrap()
        .flatten()
        .filter_map(|e| std::fs::read_to_string(e.path()).ok())
        .find(|text| text.contains("kind = \"task\""))
        .expect("a task item");
    assert!(item.contains("event = \"PR merged, task open\""), "{item}");
    assert!(
        item.contains("t-0001's pull request merged and TASKS.md still has \\\"Ship checkout\\\" open on it (line 4)"),
        "{item}"
    );
}

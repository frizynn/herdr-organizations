//! The `--json` contract (docs/json.md), checked as whole documents against the
//! built binary. A failing comparison here means the contract changed: bump
//! `schema_version` when a field is renamed, removed or changes meaning.

#![recursion_limit = "256"]

use std::path::Path;
use std::process::Command;

use serde_json::{Value, json};

const BIN: &str = env!("CARGO_BIN_EXE_herdr-organizations");
const HEAD: &str = "0123456789abcdef0123456789abcdef01234567";
const URL: &str = "https://github.com/owner/app/pull/7";

fn hp(home: &Path, args: &[&str]) -> String {
    let out = Command::new(BIN)
        .env_clear()
        .env("HOME", home)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

fn hp_json(home: &Path, args: &[&str]) -> Value {
    let text = hp(home, args);
    assert_eq!(text.lines().count(), 1, "one JSON line: {text}");
    serde_json::from_str(&text).unwrap()
}

struct Fixture {
    home: tempfile::TempDir,
    root: String,
}

impl Fixture {
    fn args<'a>(&'a self, rest: &[&'a str]) -> Vec<&'a str> {
        let mut args = vec!["--root", self.root.as_str()];
        args.extend_from_slice(rest);
        args
    }

    fn json(&self, rest: &[&str]) -> Value {
        hp_json(self.home.path(), &self.args(rest))
    }

    fn text(&self, rest: &[&str]) -> String {
        hp(self.home.path(), &self.args(rest))
    }

    fn dir(&self) -> String {
        format!("{}/demo", self.root)
    }
}

/// A project with a coordinator node that has a pull request and an unread
/// report, and a resolved worker under it. The project was never opened, so
/// every view falls back to the recorded group.
fn fixture() -> Fixture {
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join("root").to_str().unwrap().to_string();
    let fixture = Fixture { home, root };
    let created = fixture.json(&["new", "Demo", "--goal", "Ship it", "--json"]);
    assert_eq!(created["schema_version"], 1);
    let dir = Path::new(&fixture.root).join("demo");
    std::fs::write(
        dir.join("threads/t-0001.toml"),
        format!(
            r#"id = "t-0001"
title = "Checkout"
role = "coordinator"
can_spawn = true
status = "open"
kind = "worktree"
repo = "/repo"
origin = "git@github.com:owner/app.git"
branch = "hp/demo/t-0001-checkout"
base = "main"
workspace_id = "w2"
tab_id = "w2:t1"
pane_id = "w2:p1"
agent = "codex"
model = "gpt-5"
agent_name = "hp-demo-t-0001"
cwd = "/work/checkout"
worktree_path = "/work/checkout"
created = "2026-10-01T10:00:00Z"
updated = "2026-10-01T11:00:00Z"
last_state = "idle"
last_state_change = "2026-10-01T10:30:00Z"
last_group = "ready-for-review"
report_hash = "new"
acked_report_hash = "old"
pr = "{URL}"
pr_state = "OPEN"
pr_review = "APPROVED"
auto_fix_ci = true
"#
        ),
    )
    .unwrap();
    std::fs::write(
        dir.join("threads/t-0002.toml"),
        r#"id = "t-0002"
title = "Copy fix"
parent_id = "t-0001"
status = "resolved"
kind = "tab"
created = "2026-10-01T10:05:00Z"
updated = "2026-10-01T10:50:00Z"
last_group = "resolved"
resolved_reason = "merged"
"#,
    )
    .unwrap();
    std::fs::write(
        dir.join(".state/ticker.json"),
        json!({
            "prs": {"t-0001": {
                "state": "OPEN", "review_decision": "APPROVED",
                "failing_checks": [], "comment_count": 2, "commenters": ["alice"],
                "checks": {"passed": 4, "pending": 1, "failed": 0},
                "additions": 120, "deletions": 8, "is_draft": false,
                "mergeable": "MERGEABLE", "head_oid": HEAD
            }}
        })
        .to_string(),
    )
    .unwrap();
    std::fs::write(
        dir.join("inbox/20261001T100000Z-pr-t-0001-1.md"),
        "+++\nid = \"20261001T100000Z-pr-t-0001-1\"\nkind = \"pr\"\nsubject = \"t-0001\"\ncreated = \"2026-10-01T10:00:00Z\"\nsummary = \"t-0001 \\\"Checkout\\\": pull request state OPEN\"\n+++\n",
    )
    .unwrap();
    fixture
}

fn checkout_thread(fixture: &Fixture) -> Value {
    json!({
        "id": "t-0001",
        "title": "Checkout",
        "parent_id": "root",
        "role": "coordinator",
        "can_spawn": true,
        "status": "open",
        "kind": "worktree",
        "group": "ready-for-review",
        "group_label": "Ready for review",
        "rank": 2,
        "note": "session unreachable",
        "agent": "codex",
        "model": "gpt-5",
        "reasoning_effort": "",
        "agent_name": "hp-demo-t-0001",
        "workspace_id": "w2",
        "tab_id": "w2:t1",
        "pane_id": "w2:p1",
        "machine": "",
        "cwd": "/work/checkout",
        "repo": "/repo",
        "branch": "hp/demo/t-0001-checkout",
        "base": "main",
        "worktree_path": "/work/checkout",
        "has_report": true,
        "report_unacked": true,
        "created": "2026-10-01T10:00:00Z",
        "updated": "2026-10-01T11:00:00Z",
        "last_state": "idle",
        "last_state_change": "2026-10-01T10:30:00Z",
        "error": "",
        "resolved_reason": "",
        "auto_fix_ci": true,
        "auto_merge": false,
        "pr": {
            "url": URL,
            "state": "OPEN",
            "review": "APPROVED",
            "checks": {"passed": 4, "pending": 1, "failed": 0},
            "additions": 120,
            "deletions": 8,
            "failing": [],
            "comment_count": 2,
            "draft": false,
            "mergeable": "MERGEABLE",
            "merge_blocker": "1 check(s) have not finished"
        },
        "profile": "",
        "origin": "git@github.com:owner/app.git",
        "state_line": "",
        "activity": "",
        "percent": null,
        "next": [],
        "report": null,
        "library": format!("{}/library/t-0001", fixture.dir())
    })
}

fn copy_fix_thread(fixture: &Fixture) -> Value {
    json!({
        "id": "t-0002",
        "title": "Copy fix",
        "parent_id": "t-0001",
        "role": "worker",
        "can_spawn": false,
        "status": "resolved",
        "kind": "tab",
        "group": "resolved",
        "group_label": "Resolved",
        "rank": 6,
        "note": "merged",
        "agent": "",
        "model": "",
        "reasoning_effort": "",
        "agent_name": "",
        "workspace_id": "",
        "tab_id": "",
        "pane_id": "",
        "machine": "",
        "cwd": "",
        "repo": "",
        "branch": "",
        "base": "",
        "worktree_path": "",
        "has_report": false,
        "report_unacked": false,
        "created": "2026-10-01T10:05:00Z",
        "updated": "2026-10-01T10:50:00Z",
        "last_state": "",
        "last_state_change": "",
        "error": "",
        "resolved_reason": "merged",
        "auto_fix_ci": false,
        "auto_merge": false,
        "pr": null,
        "profile": "",
        "origin": "",
        "state_line": "",
        "activity": "",
        "percent": null,
        "next": [],
        "report": null,
        "library": format!("{}/library/t-0002", fixture.dir())
    })
}

fn project(fixture: &Fixture) -> Value {
    json!({
        "slug": "demo",
        "name": "Demo",
        "goal": "Ship it",
        "status": "active",
        "dir": fixture.dir(),
        "counts": {"ready-for-review": 1, "resolved": 1}
    })
}

#[test]
fn thread_list_json_is_the_documented_document() {
    let fixture = fixture();
    assert_eq!(
        fixture.json(&["thread", "list", "demo", "--json"]),
        json!({
            "schema_version": 1,
            "project": "demo",
            "threads": [checkout_thread(&fixture), copy_fix_thread(&fixture)]
        })
    );
}

#[test]
fn node_list_json_adds_tree_position() {
    let fixture = fixture();
    let mut root = checkout_thread(&fixture);
    root["depth"] = json!(1);
    root["tree_order"] = json!(1);
    let mut child = copy_fix_thread(&fixture);
    child["depth"] = json!(2);
    child["tree_order"] = json!(2);
    assert_eq!(
        fixture.json(&["node", "list", "demo", "--json"]),
        json!({"schema_version": 1, "project": "demo", "nodes": [root, child]})
    );
}

#[test]
fn project_list_overview_and_new_share_the_project_shape() {
    let fixture = fixture();
    assert_eq!(
        fixture.json(&["list", "--json"]),
        json!({"schema_version": 1, "projects": [project(&fixture)]})
    );
    let mut with_threads = project(&fixture);
    with_threads["threads"] = json!([checkout_thread(&fixture), copy_fix_thread(&fixture)]);
    let expected = json!({"schema_version": 1, "projects": [with_threads]});
    assert_eq!(fixture.json(&["overview", "demo", "--json"]), expected);
    // No slug and no Herdr workspace: every project, never a picker.
    assert_eq!(fixture.json(&["overview", "--json"]), expected);

    let created = fixture.json(&["new", "Other", "--json"]);
    assert_eq!(
        created,
        json!({
            "schema_version": 1,
            "project": {
                "slug": "other",
                "name": "Other",
                "goal": "",
                "status": "active",
                "dir": format!("{}/other", fixture.root),
                "counts": {}
            },
            "next": format!("{BIN} --root {} open other", fixture.root)
        })
    );
}

#[test]
fn inbox_list_json_reads_without_consuming() {
    let fixture = fixture();
    let expected = json!({
        "schema_version": 1,
        "project": "demo",
        "items": [{
            "id": "20261001T100000Z-pr-t-0001-1",
            "kind": "pr",
            "subject": "t-0001",
            "created": "2026-10-01T10:00:00Z",
            "summary": "t-0001 \"Checkout\": pull request state OPEN",
            "body": "",
            "seen": false
        }]
    });
    assert_eq!(fixture.json(&["inbox", "list", "demo", "--json"]), expected);
    assert_eq!(fixture.json(&["inbox", "list", "demo", "--json"]), expected);
    assert!(
        !Path::new(&fixture.dir())
            .join(".state/inbox-seen.json")
            .exists()
    );
}

#[test]
fn text_output_is_unchanged() {
    let fixture = fixture();
    assert_eq!(
        fixture.text(&["list"]),
        "demo\tactive\tReady for review: 1, Resolved: 1\n"
    );
    assert_eq!(
        fixture.text(&["thread", "list", "demo"]),
        "t-0001\tReady for review\tsession unreachable\tCheckout\nt-0002\tResolved\tmerged\tCopy fix\n"
    );
    assert_eq!(
        fixture.text(&["node", "list", "demo"]),
        "└─ t-0001\tcoordinator\tparent=root\tReady for review\tCheckout\n   └─ t-0002\tworker\tparent=t-0001\tResolved\tCopy fix\n"
    );
    let overview = fixture.text(&["overview", "demo"]);
    assert!(
        overview.starts_with("demo (active): Ship it\n"),
        "{overview}"
    );
    assert!(overview.contains("\nReady for review (1)\n  t-0001  Checkout  [session unreachable]  hp/demo/t-0001-checkout\n"), "{overview}");
}

#[test]
fn a_pull_request_the_ticker_has_not_read_has_no_invented_numbers() {
    let fixture = fixture();
    std::fs::remove_file(Path::new(&fixture.dir()).join(".state/ticker.json")).unwrap();
    let listed = fixture.json(&["thread", "list", "demo", "--json"]);
    assert_eq!(
        listed["threads"][0]["pr"],
        json!({
            "url": URL,
            "state": "OPEN",
            "review": "APPROVED",
            "checks": null,
            "additions": null,
            "deletions": null,
            "failing": [],
            "comment_count": null,
            "draft": null,
            "mergeable": null,
            "merge_blocker": "the ticker has not read this pull request yet"
        })
    );
}

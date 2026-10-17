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

/// What `new` writes to a project's STATUS.md.
const NEW_STATUS: &str =
    "# Status\nObjective: (none)\n## Asked\n## Needs your decision\n## Decisions\n";
const ROOT_STATUS: &str =
    "# Status\nObjective: Ship it\n## Asked\n- [open] Is it in the skill? : checking (t-0001)\n";
const NODE_STATUS: &str =
    "# Status\nObjective: Checkout\n## In progress\n- t-0002 copy fix : merged\n";

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
task = "Checkout"
task_origin = "task"
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
    std::fs::write(dir.join("STATUS.md"), ROOT_STATUS).unwrap();
    touch(&dir.join("STATUS.md"), "2026-10-02T08:00:00Z");
    std::fs::create_dir_all(dir.join("nodes/t-0001")).unwrap();
    std::fs::write(dir.join("nodes/t-0001/STATUS.md"), NODE_STATUS).unwrap();
    touch(&dir.join("nodes/t-0001/STATUS.md"), "2026-10-02T09:00:00Z");
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
            "merge_blocker": "1 check(s) have not finished",
            "actions": ["address-comments"]
        },
        "actions": [],
        "profile": "",
        "origin": "git@github.com:owner/app.git",
        "state_line": "",
        "activity": "",
        "percent": null,
        "next": [],
        "report": null,
        "library": format!("{}/library/t-0001", fixture.dir()),
        "verdict": null,
        "summary": null,
        "remember": [],
        "captures": [],
        "deliverables": [],
        "task": "Checkout",
        "task_origin": "task",
        "usage": null,
        "status_file": {
            "path": format!("{}/nodes/t-0001/STATUS.md", fixture.dir()),
            "text": NODE_STATUS,
            "updated": "2026-10-02T09:00:00Z",
            "objective": "Checkout"
        }
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
        "actions": [],
        "profile": "",
        "origin": "",
        "state_line": "",
        "activity": "",
        "percent": null,
        "next": [],
        "report": null,
        "library": format!("{}/library/t-0002", fixture.dir()),
        "verdict": null,
        "summary": null,
        "remember": [],
        "captures": [],
        "deliverables": [],
        "task": "",
        "task_origin": "",
        "usage": null,
        "status_file": null
    })
}

fn project(fixture: &Fixture) -> Value {
    json!({
        "slug": "demo",
        "name": "Demo",
        "goal": "Ship it",
        "status": "active",
        "dir": fixture.dir(),
        "counts": {"ready-for-review": 1, "resolved": 1},
        "icon": "",
        "tone": "",
        "usage": null,
        "status_file": {
            "path": format!("{}/STATUS.md", fixture.dir()),
            "text": ROOT_STATUS,
            "updated": "2026-10-02T08:00:00Z",
            "objective": "Ship it",
            "asks": [root_ask()]
        }
    })
}

/// The one ask of `ROOT_STATUS`: it links the coordinator node, whose pull
/// request is open, so it is in progress.
fn root_ask() -> Value {
    json!({
        "line": 4,
        "state": "open",
        "text": "Is it in the skill?",
        "answer": "checking",
        "links": [{
            "kind": "thread", "text": "t-0001", "target": "t-0001",
            "thread": "t-0001", "title": "Checkout", "state": "ready-for-review",
            "pr": URL, "pr_state": "open"
        }],
        "confirmed": "in-progress",
        "mismatch": ""
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

    let mut created = fixture.json(&["new", "Other", "--json"]);
    // A new project starts with an empty STATUS.md, written just now.
    let updated = created["project"]["status_file"]["updated"].take();
    assert!(
        updated.as_str().unwrap().parse::<jiff::Timestamp>().is_ok(),
        "{updated}"
    );
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
                "counts": {},
                "icon": "",
                "tone": "",
                "usage": null,
                "status_file": {
                    "path": format!("{}/other/STATUS.md", fixture.root),
                    "text": NEW_STATUS,
                    "updated": null
                }
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
            "seen": false,
            "audience": "coordinator"
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

/// Every item says who it is for. One written before `audience` is the
/// coordinator's, as it always was.
#[test]
fn inbox_list_json_says_who_each_item_is_for() {
    let fixture = fixture();
    write(
        &fixture,
        "inbox/20261001T100001Z-pr-t-0001-2.md",
        "+++\nid = \"20261001T100001Z-pr-t-0001-2\"\nkind = \"pr\"\nsubject = \"t-0001\"\ncreated = \"2026-10-01T10:00:01Z\"\nsummary = \"auto-fix asked it\"\nevent = \"auto-fix\"\naudience = \"user\"\n+++\n",
    );
    let listed = fixture.json(&["inbox", "list", "demo", "--json"]);
    let audiences: Vec<(&str, &str)> = listed["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| {
            (
                item["id"].as_str().unwrap(),
                item["audience"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        audiences,
        [
            ("20261001T100000Z-pr-t-0001-1", "coordinator"),
            ("20261001T100001Z-pr-t-0001-2", "user")
        ]
    );
    // `context` leaves the user's item out and says how many there are.
    let context = fixture.text(&["context", "demo", "--peek"]);
    assert!(!context.contains("auto-fix asked it"), "{context}");
    assert!(context.contains("## Inbox (1 unhandled)"), "{context}");
    assert!(
        context.contains("\n1 more item(s) for the user, not for you: leave them.\n"),
        "{context}"
    );
}

#[test]
fn node_show_names_a_coordinator_nodes_status_file() {
    let fixture = fixture();
    let status = format!("{}/nodes/t-0001/STATUS.md", fixture.dir());
    std::fs::remove_file(&status).unwrap();
    let shown = fixture.text(&["node", "show", "demo", "t-0001"]);
    assert!(
        shown.contains(&format!("# status: {status} (not written yet)\n")),
        "{shown}"
    );
    write(&fixture, "nodes/t-0001/STATUS.md", "# Status\n");
    let shown = fixture.text(&["node", "show", "demo", "t-0001"]);
    assert!(shown.contains(&format!("# status: {status}\n")), "{shown}");
    let worker = fixture.text(&["node", "show", "demo", "t-0002"]);
    assert!(!worker.contains("# status"), "{worker}");
}

/// What a thread's report says and shows: the verdict from its `Status:`
/// line, its `## Summary`, the images it lists under `## Captures` and the
/// documents at the top of its library. `overview` carries the same.
#[test]
fn a_thread_carries_its_verdict_summary_captures_and_deliverables() {
    let fixture = fixture();
    let library = format!("{}/library/t-0002", fixture.dir());
    write(
        &fixture,
        "threads/t-0002.md",
        "PR: https://github.com/owner/app/pull/9\nStatus: done\n## Summary\nSizes are in order at 390 px.\nCoupons are untouched.\n## Report\nLong text.\n## Captures\n- capturas/talles-390.png \u{2014} Orden de talles a 390 px\n- capturas/missing.png - Gone\n- ../t-0001/x.png - Escapes\n## Next\n- Merge the PR\n## Remember\n- Sizes sort by the size table, not by name.\n",
    );
    for file in [
        "library/t-0002/capturas/talles-390.png",
        "library/t-0002/informe.html",
        "library/t-0002/notes.txt",
        "library/t-0001/x.png",
    ] {
        write(&fixture, file, "x");
    }
    let shown = fixture.json(&["thread", "show", "demo", "t-0002", "--json"]);
    let thread = &shown["thread"];
    assert_eq!(thread["verdict"], "done");
    assert_eq!(
        thread["summary"],
        "Sizes are in order at 390 px.\nCoupons are untouched."
    );
    assert_eq!(
        thread["remember"],
        json!(["Sizes sort by the size table, not by name."])
    );
    assert_eq!(
        thread["captures"],
        json!([{
            "path": format!("{library}/capturas/talles-390.png"),
            "caption": "Orden de talles a 390 px"
        }])
    );
    assert_eq!(
        thread["deliverables"],
        json!([{"path": format!("{library}/informe.html"), "kind": "html"}])
    );
    let overview = fixture.json(&["overview", "demo", "--json"]);
    let listed = &overview["projects"][0]["threads"][1];
    assert_eq!(listed["id"], "t-0002");
    for field in ["verdict", "summary", "remember", "captures", "deliverables"] {
        assert_eq!(listed[field], thread[field], "{field}");
    }
    // `node show`, which a parent nudge names, prints the verdict and summary.
    let shown = fixture.text(&["node", "show", "demo", "t-0002"]);
    assert!(
        shown.contains(
            "# verdict: done\n# summary:\n#   Sizes are in order at 390 px.\n#   Coupons are untouched.\n# remember:\n#   Sizes sort by the size table, not by name.\n"
        ),
        "{shown}"
    );

    // Any other Status value is no verdict.
    write(
        &fixture,
        "threads/t-0002.md",
        "Status: mostly done\n## Report\n",
    );
    let shown = fixture.json(&["thread", "show", "demo", "t-0002", "--json"]);
    assert_eq!(shown["thread"]["verdict"], Value::Null);
    assert_eq!(shown["thread"]["summary"], Value::Null);
    assert_eq!(shown["thread"]["remember"], json!([]));
}

/// A `Status:` line in any case, or with `PASS` or `ISSUES` as other
/// harnesses write it, is a verdict: `PASS` is `done`, `ISSUES` is `issues`.
#[test]
fn a_status_line_in_any_case_or_with_pass_or_issues_is_a_verdict() {
    let fixture = fixture();
    for (status, verdict, words) in [
        ("PASS", "done", "done"),
        ("ISSUES", "issues", "issues"),
        ("Blocked", "blocked", "blocked"),
        ("FAIL", "failed", "failed"),
    ] {
        write(
            &fixture,
            "threads/t-0002.md",
            &format!("Status: {status}\n## Summary\nChecked.\n"),
        );
        let shown = fixture.json(&["thread", "show", "demo", "t-0002", "--json"]);
        assert_eq!(shown["thread"]["verdict"], verdict, "{status}");
        let shown = fixture.text(&["node", "show", "demo", "t-0002"]);
        assert!(
            shown.contains(&format!("# verdict: {words}\n# summary:\n#   Checked.\n")),
            "{shown}"
        );
    }
}

/// A coordinator node's report is only its verdict, its summary and where
/// its status file is: no `## Report` and no `## Next`, so no Next list.
#[test]
fn a_coordinator_nodes_short_report_has_no_next_list() {
    let fixture = fixture();
    write(
        &fixture,
        "threads/t-0002.md",
        "Status: done\n## Summary\nLogin and docs shipped.\nDetails: nodes/t-0002/STATUS.md\n",
    );
    let thread = &fixture.json(&["thread", "show", "demo", "t-0002", "--json"])["thread"];
    assert_eq!(thread["verdict"], "done");
    assert_eq!(
        thread["summary"],
        "Login and docs shipped.\nDetails: nodes/t-0002/STATUS.md"
    );
    assert_eq!(thread["next"], json!([]));
    assert_eq!(
        fixture.text(&["thread", "next", "demo", "t-0002"]),
        "t-0002 has no Next list\n"
    );
}

/// STATUS.md is the root's only status file: without it `status_file` is
/// null, even when a HANDOFF.md from before the rename is there.
#[test]
fn the_root_status_file_is_null_without_status_md() {
    let fixture = fixture();
    let dir = Path::new(&fixture.dir()).to_path_buf();
    std::fs::remove_file(dir.join("STATUS.md")).unwrap();
    std::fs::write(dir.join("HANDOFF.md"), "# Handoff\n").unwrap();
    let listed = fixture.json(&["list", "--json"]);
    assert_eq!(listed["projects"][0]["status_file"], Value::Null);

    // A status file is shown up to 4,000 characters, never through a link.
    std::fs::write(dir.join("STATUS.md"), "x".repeat(5_000)).unwrap();
    let shown = fixture.json(&["list", "--json"]);
    let text = shown["projects"][0]["status_file"]["text"]
        .as_str()
        .unwrap();
    assert_eq!(text.chars().count(), 4_000);
    let node = dir.join("nodes/t-0001/STATUS.md");
    std::fs::remove_file(&node).unwrap();
    std::os::unix::fs::symlink(dir.join("STATUS.md"), &node).unwrap();
    let threads = fixture.json(&["thread", "list", "demo", "--json"]);
    assert_eq!(threads["threads"][0]["status_file"], Value::Null);
}

/// With `records` set, the reports, task files, Next lines, libraries and
/// status files move there, and every view reads them there; clearing it
/// brings them back.
#[test]
fn records_move_where_project_md_says_and_every_view_reads_them_there() {
    let fixture = fixture();
    write(
        &fixture,
        "threads/t-0002.md",
        "Status: done\n## Summary\nShipped.\n## Next\n- Merge\n",
    );
    write(&fixture, "threads/t-0002.next.md", "- Tell the user\n");
    write(&fixture, "threads/t-0002.task.md", "Fix the copy.\n");
    write(&fixture, "library/t-0002/informe.html", "x");
    let dir = Path::new(&fixture.dir()).to_path_buf();
    let vault = fixture.home.path().join("vault/demo");
    let v = vault.display().to_string();
    // `~/` is the caller's home; `set` says the folder it stored.
    let set = fixture.text(&["set", "demo", "records", "~/vault/demo"]);
    assert!(set.starts_with(&format!("demo: records = {v}\n")), "{set}");
    for moved in [
        "threads/t-0002.md",
        "threads/t-0002.next.md",
        "threads/t-0002.task.md",
        "library/t-0002/informe.html",
        "STATUS.md",
        "nodes/t-0001/STATUS.md",
    ] {
        assert!(vault.join(moved).is_file(), "{moved}");
        assert!(!dir.join(moved).exists(), "{moved}");
    }
    assert!(dir.join("threads/t-0002.toml").is_file());
    assert_eq!(
        std::fs::read_to_string(vault.join("STATUS.md")).unwrap(),
        ROOT_STATUS
    );

    let thread = &fixture.json(&["thread", "show", "demo", "t-0002", "--json"])["thread"];
    assert_eq!(thread["report"], format!("{v}/threads/t-0002.md"));
    assert_eq!(thread["library"], format!("{v}/library/t-0002"));
    assert_eq!(thread["verdict"], "done");
    assert_eq!(thread["summary"], "Shipped.");
    assert_eq!(thread["next"], json!(["Merge", "Tell the user"]));
    assert_eq!(
        thread["deliverables"],
        json!([{"path": format!("{v}/library/t-0002/informe.html"), "kind": "html"}])
    );
    let node = &fixture.json(&["node", "show", "demo", "t-0001", "--json"])["thread"];
    assert_eq!(
        node["status_file"],
        json!({
            "path": format!("{v}/nodes/t-0001/STATUS.md"),
            "text": NODE_STATUS,
            "updated": "2026-10-02T09:00:00Z",
            "objective": "Checkout"
        })
    );
    let shown = fixture.text(&["node", "show", "demo", "t-0001"]);
    assert!(
        shown.contains(&format!("# status: {v}/nodes/t-0001/STATUS.md\n")),
        "{shown}"
    );
    let project = &fixture.json(&["project", "show", "demo", "--json"])["project"];
    assert_eq!(project["records"], v);
    assert_eq!(
        project["status_file"],
        json!({
            "path": format!("{v}/STATUS.md"),
            "text": ROOT_STATUS,
            "updated": "2026-10-02T08:00:00Z",
            "objective": "Ship it",
            "asks": [root_ask()]
        })
    );
    let status = fixture.json(&["project", "status", "demo", "--json"]);
    assert_eq!(
        status["sources"]["status_file"],
        format!("{v}/STATUS.md"),
        "where the root's status file is"
    );
    let context = fixture.text(&["context", "demo", "--peek"]);
    assert!(
        context.contains(&format!(
            "## Status ({v}/STATUS.md)\n# Status\nObjective: Ship it"
        )),
        "{context}"
    );
    assert!(context.contains(&format!(" records={v}")), "{context}");

    fixture.text(&["thread", "next", "demo", "t-0002", "--add", "Close it"]);
    assert_eq!(
        std::fs::read_to_string(vault.join("threads/t-0002.next.md")).unwrap(),
        "- Tell the user\n- Close it\n"
    );

    fixture.text(&["set", "demo", "records", ""]);
    assert!(dir.join("threads/t-0002.next.md").is_file());
    assert!(!vault.join("threads/t-0002.next.md").exists());
    let thread = &fixture.json(&["thread", "show", "demo", "t-0002", "--json"])["thread"];
    assert_eq!(
        thread["report"],
        format!("{}/threads/t-0002.md", fixture.dir())
    );
}

/// A project that was opened names its coordinator as the ticker last saw
/// it; one that never was has no `coordinator`.
#[test]
fn a_project_names_its_coordinator() {
    let fixture = fixture();
    let listed = fixture.json(&["list", "--json"]);
    assert_eq!(listed["projects"][0].get("coordinator"), None);
    write(
        &fixture,
        ".state/coordinator.json",
        r#"{"pane_id": "w1:p1", "agent": "claude", "model": "opus", "profile": "claude"}"#,
    );
    let listed = fixture.json(&["list", "--json"]);
    assert_eq!(
        listed["projects"][0]["coordinator"],
        json!({"pane_id": "w1:p1", "agent": "claude", "model": "opus", "state": ""})
    );
    write(
        &fixture,
        ".state/coordinators.json",
        r#"[{"pane_id": "w9:p9", "agent_status": "idle"}, {"pane_id": "w1:p1", "agent_status": "working"}]"#,
    );
    let shown = fixture.json(&["overview", "demo", "--json"]);
    assert_eq!(shown["projects"][0]["coordinator"]["state"], "working");
}

/// Every coordinator's asks, the root's and a node's, of two projects: what
/// waits on the user first, then what does not close, what is in progress
/// and what is done, each in project, coordinator and line order. A done ask on a thread whose pull request is open does
/// not close.
#[test]
fn asks_json_lists_every_coordinators_asks_in_order() {
    let fixture = fixture();
    write(
        &fixture,
        "STATUS.md",
        "# Status\nObjective: Ship it\n## Asked\n- [done] Ship the checkout : merged (t-0001)\n- [needs-decision] Pilot or not : yes or no\n- [answered] Is it fast? : yes, measured\n## Needs your decision\n- Pilot : yes or no\n",
    );
    write(
        &fixture,
        "nodes/t-0001/STATUS.md",
        "# Status\nObjective: Checkout\n## Asked\n- [working] Copy fix : landing it (t-0002)\n- [open] Docs : next\n",
    );
    fixture.text(&["new", "Beta"]);
    std::fs::write(
        Path::new(&fixture.root).join("beta/STATUS.md"),
        "# Status\n## Asked\n- [working] Research : reading (https://example.com/notes)\n- [needs-decision] Budget : pick one\n",
    )
    .unwrap();

    let listed = fixture.json(&["asks", "--json"]);
    assert_eq!(listed["schema_version"], 1);
    let rows: Vec<(String, String, String, String, String, String)> = listed["asks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| {
            let s = |v: &Value| v.as_str().unwrap().to_string();
            (
                s(&a["project"]),
                s(&a["coordinator"]["id"]),
                s(&a["state"]),
                s(&a["confirmed"]),
                s(&a["mismatch"]),
                s(&a["text"]),
            )
        })
        .collect();
    let row = |p: &str, c: &str, state: &str, confirmed: &str, mismatch: &str, text: &str| {
        let s = |v: &str| v.to_string();
        (s(p), s(c), s(state), s(confirmed), s(mismatch), s(text))
    };
    assert_eq!(
        rows,
        [
            row("beta", "root", "needs-decision", "", "", "Budget"),
            row("demo", "root", "needs-decision", "", "", "Pilot or not"),
            row(
                "demo",
                "root",
                "done",
                "in-progress",
                "a linked PR is still open",
                "Ship the checkout"
            ),
            row(
                "demo",
                "t-0001",
                "working",
                "done",
                "a linked thread is resolved",
                "Copy fix"
            ),
            row("beta", "root", "working", "", "", "Research"),
            row("demo", "t-0001", "open", "", "", "Docs"),
            row("demo", "root", "answered", "", "", "Is it fast?"),
        ]
    );
    let first = &listed["asks"][2];
    assert_eq!(first["project"], "demo");
    assert_eq!(first["coordinator"], json!({"id": "root", "title": ""}));
    assert_eq!(first["line"], 4);
    assert_eq!(first["answer"], "merged");
    assert_eq!(first["links"][0]["pr_state"], "open");
    assert_eq!(
        listed["asks"][3]["coordinator"],
        json!({"id": "t-0001", "title": "Checkout"})
    );

    let one = fixture.json(&["asks", "--project", "beta", "--json"]);
    assert_eq!(one["asks"].as_array().unwrap().len(), 2);
    let text = fixture.text(&["asks", "--project", "demo"]);
    assert_eq!(
        text.lines().next().unwrap(),
        "demo\troot\tneeds-decision\t\t\tPilot or not : yes or no\t"
    );
    assert!(
        text.contains("demo\troot\tdone\tin-progress\ta linked PR is still open\tShip the checkout : merged\tt-0001 ready-for-review (PR open)\n"),
        "{text}"
    );
    assert!(
        text.contains("demo\tt-0001 Checkout\tworking\tdone\ta linked thread is resolved\tCopy fix : landing it\tt-0002 resolved\n"),
        "{text}"
    );
}

/// `context` names the root's asks that do not close by their line and a
/// reason of the binary's, never by their text; `project status` and `task
/// lint` keep to the files they check.
#[test]
fn context_lists_the_asks_that_do_not_close() {
    let fixture = fixture();
    write(
        &fixture,
        "STATUS.md",
        "# Status\nObjective: Ship it\n## Asked\n- [done] SECRET-TEXT : merged (t-0001)\n- [working] Docs : writing\n- [open] Fine : waiting (t-0001)\n",
    );
    let context = fixture.text(&["context", "demo", "--peek"]);
    let drift = context.split_once("## Drift").unwrap().1;
    assert!(
        drift.contains(
            "- [ask-mismatch] STATUS.md:4 is marked done, but a linked PR is still open\n"
        ),
        "{drift}"
    );
    assert!(
        drift.contains(
            "- [ask-mismatch] STATUS.md:5 is marked working, but no thread, PR or task is linked\n"
        ),
        "{drift}"
    );
    assert!(!drift.contains("SECRET-TEXT"), "{drift}");
    assert!(!drift.contains("STATUS.md:6"), "{drift}");
    let status = fixture.json(&["project", "status", "demo", "--json"]);
    assert_eq!(status["problems"], json!([]));
    assert_eq!(status["drift"].get("ask_mismatch"), None);
    assert_eq!(fixture.text(&["task", "lint", "demo"]), "");
}

/// A project whose status file has no asks lists none, and its documents
/// are what they were.
#[test]
fn a_project_without_asks_lists_none() {
    let fixture = fixture();
    write(&fixture, "STATUS.md", NEW_STATUS);
    write(&fixture, "nodes/t-0001/STATUS.md", NEW_STATUS);
    assert_eq!(
        fixture.json(&["asks", "--json"]),
        json!({"schema_version": 1, "asks": []})
    );
    assert_eq!(fixture.text(&["asks"]), "");
    let listed = fixture.json(&["list", "--json"]);
    let keys: Vec<&String> = listed["projects"][0]["status_file"]
        .as_object()
        .unwrap()
        .keys()
        .collect();
    assert_eq!(keys, ["path", "text", "updated"]);
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
            "merge_blocker": "the ticker has not read this pull request yet",
            "actions": []
        })
    );
}

#[test]
fn template_list_json_is_the_bare_array_web_clients_read() {
    let fixture = fixture();
    let scope = Path::new(&fixture.dir()).join("nodes/t-0001");
    std::fs::create_dir_all(scope.join("memory")).unwrap();
    std::fs::write(
        scope.join("INSTRUCTIONS.md"),
        "# Node instructions\n\nShip small PRs.\n",
    )
    .unwrap();
    let memory = "# Node memory\n\n- Use pnpm.\n";
    std::fs::write(scope.join("MEMORY.md"), memory).unwrap();
    fixture.text(&[
        "template",
        "save",
        "demo",
        "t-0001",
        "--name",
        "checkout",
        "--description",
        "Checkout lead",
    ]);

    let listed = fixture.json(&["template", "list", "--project", "demo", "--json"]);
    let row = json!({
        "name": "checkout",
        "scope": "project",
        "project": "demo",
        "description": "Checkout lead",
        "role": "coordinator",
        "can_spawn": true,
        "profile": "codex",
        "harness": "codex",
        "model": "",
        "reasoning_effort": "",
        "permission_profile": "",
        "rules_chars": "Ship small PRs.".len(),
        "memory_chars": memory.len(),
        "updated": listed[0]["updated"],
        "dir": format!("{}/templates/checkout", fixture.dir()),
    });
    assert_eq!(listed, json!([row]));
    assert!(listed[0]["updated"].as_str().unwrap().ends_with('Z'));

    let mut shown = row;
    shown["rules"] = "Ship small PRs.\n".into();
    shown["memory"] = memory.into();
    shown["memory_files"] = json!([]);
    assert_eq!(
        fixture.json(&[
            "template",
            "show",
            "checkout",
            "--project",
            "demo",
            "--json"
        ]),
        json!({"schema_version": 1, "template": shown})
    );
}

fn write(fixture: &Fixture, file: &str, text: &str) {
    let path = Path::new(&fixture.dir()).join(file);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

#[test]
fn project_show_json_is_the_project_with_what_every_thread_receives() {
    let fixture = fixture();
    write(
        &fixture,
        "PROJECT.md",
        r#"+++
name = "Demo"
goal = "Ship it"
coordinator_profile = "claude"
thread_profile = "codex"
max_parallel_threads = 4
auto_resolve_days = 7
nudge = true
mute = false

[[repos]]
path = "/src/app"

[[repos]]
path = "/src/api"
machine = "m1"
+++

# Instructions

Ship small PRs.
"#,
    );
    let index = "# Memory\n\n- [Payments](memory/payments.md): the provider\n";
    write(&fixture, "MEMORY.md", index);
    write(&fixture, "memory/release.md", "Release on Tuesdays.\n");
    write(&fixture, "memory/payments.md", "Use the sandbox keys.\n");
    write(&fixture, "memory/draft.txt", "not memory");
    #[cfg(unix)]
    std::os::unix::fs::symlink(
        "/etc/hosts",
        Path::new(&fixture.dir()).join("memory/linked.md"),
    )
    .unwrap();

    let mut expected = project(&fixture);
    let fields = json!({
        "instructions": "# Instructions\n\nShip small PRs.\n",
        "memory": index,
        "memory_files": [
            {"name": "payments.md", "text": "Use the sandbox keys.\n"},
            {"name": "release.md", "text": "Release on Tuesdays.\n"}
        ],
        "repos": [
            {"path": "/src/app", "machine": ""},
            {"path": "/src/api", "machine": "m1"}
        ],
        "coordinator_profile": "claude",
        "thread_profile": "codex",
        "max_parallel_threads": 4,
        "auto_resolve_days": 7,
        "nudge": true,
        "mute": false,
        "progress": true,
        "turn_reminder": true,
        "records": fixture.dir(),
        "merge_gate": "",
        "merge_method": "squash",
        "auto_merge": false
    });
    for (key, value) in fields.as_object().unwrap() {
        expected[key] = value.clone();
    }
    assert_eq!(
        fixture.json(&["project", "show", "demo", "--json"]),
        json!({"schema_version": 1, "project": expected})
    );
    let text = fixture.text(&["project", "show", "demo"]);
    assert!(
        text.ends_with(&format!(
            "repos\t/src/app, /src/api@m1\ninstructions\t32 characters\nmemory\tMEMORY.md {} characters, memory/payments.md 22 characters, memory/release.md 21 characters\n",
            index.len()
        )),
        "{text}"
    );
}

#[test]
fn task_list_json_reads_tasks_md_with_structured_owners() {
    let fixture = fixture();
    write(
        &fixture,
        "TASKS.md",
        "# Tasks\n\n## Backlog\n- [ ] Write the launch post (me)\n- [ ] Fix login (codex-fast@m1) · t-0001\n  Users on Safari get logged out.\n  See https://example.com/issue/4\n- [ ] Review the contract (Elias)\n- [ ] Call the bank (Priya Rao)\n- [ ] Old style (agent → t-0002)\n- [ ] Tidy imports (claude)\n\n## Later\n- [ ] Price page\n",
    );
    let owner = |kind: &str, profile: &str, machine: &str, name: &str| json!({"kind": kind, "profile": profile, "machine": machine, "name": name});
    let task = |list: &str, title: &str, owner: Value, thread: &str, description: &str| {
        json!({
            "list": list,
            "title": title,
            "owner": owner,
            "thread": thread,
            "description": description
        })
    };
    assert_eq!(
        fixture.json(&["task", "list", "demo", "--json"]),
        json!({
            "schema_version": 1,
            "project": "demo",
            "tasks": [
                task("Backlog", "Write the launch post", owner("me", "", "", ""), "", ""),
                task(
                    "Backlog",
                    "Fix login",
                    owner("agent", "codex-fast", "m1", ""),
                    "t-0001",
                    "Users on Safari get logged out.\nSee https://example.com/issue/4"
                ),
                task("Backlog", "Review the contract", owner("person", "", "", "Elias"), "", ""),
                task("Backlog", "Call the bank", owner("person", "", "", "Priya Rao"), "", ""),
                task("Backlog", "Old style", owner("agent", "", "", ""), "t-0002", ""),
                task("Backlog", "Tidy imports", owner("agent", "claude", "", ""), "", ""),
                task("Later", "Price page", owner("unassigned", "", "", ""), "", "")
            ]
        })
    );
    assert_eq!(
        fixture.text(&["task", "list", "demo"]),
        "Backlog\tWrite the launch post\tme\t\nBacklog\tFix login\tcodex-fast@m1\tt-0001\nBacklog\tReview the contract\tElias\t\nBacklog\tCall the bank\tPriya Rao\t\nBacklog\tOld style\tdefault profile\tt-0002\nBacklog\tTidy imports\tclaude\t\nLater\tPrice page\t\t\n"
    );

    std::fs::remove_file(Path::new(&fixture.dir()).join("TASKS.md")).unwrap();
    assert_eq!(
        fixture.json(&["task", "list", "demo", "--json"]),
        json!({"schema_version": 1, "project": "demo", "tasks": []})
    );
}

#[test]
fn routine_list_json_has_triggers_command_status_and_runs() {
    let fixture = fixture();
    write(
        &fixture,
        "routines/pr-followup.md",
        "+++\non = \"pr\"\nevents = [\"checks-failed\", \"review\"]\n+++\n\nFix what the reviewers asked.\n",
    );
    write(
        &fixture,
        "routines/standup.md",
        "+++\nschedule = \"every 2h\"\n+++\nSummarize the open threads.\n",
    );
    write(
        &fixture,
        "routines/deps.md",
        "+++\nschedule = \"daily 03:00\"\ncommand = \"cargo outdated\"\nenabled = false\n+++\nReport outdated crates.\n",
    );
    write(&fixture, "routines/broken.md", "no front matter\n");
    write(
        &fixture,
        ".state/ticker.json",
        &json!({"routines": {
            "standup": {"last_run": "2026-10-01T09:00:00.25Z", "skipped": 1, "no_coordinator": 2},
            // Left from when pr-followup was a scheduled routine.
            "pr-followup": {"last_run": "2026-09-01T09:00:00Z", "skipped": 3}
        }})
        .to_string(),
    );
    let routine = |name: &str, trigger: &str, schedule: &str, events: Value, enabled: bool| {
        json!({
            "name": name,
            "trigger": trigger,
            "schedule": schedule,
            "events": events,
            "enabled": enabled,
            "command": "",
            "command_status": "none",
            "prompt": "",
            "last_run": "",
            "next_run": "",
            "skipped_unhandled": 0,
            "skipped_no_coordinator": 0
        })
    };
    let mut deps = routine("deps", "schedule", "daily 03:00", json!([]), false);
    deps["command"] = "cargo outdated".into();
    deps["command_status"] = "commands-off".into();
    deps["prompt"] = "Report outdated crates.".into();
    let mut followup = routine(
        "pr-followup",
        "pr",
        "",
        json!(["checks-failed", "review"]),
        true,
    );
    followup["prompt"] = "Fix what the reviewers asked.".into();
    let mut standup = routine("standup", "schedule", "every 2h", json!([]), true);
    standup["prompt"] = "Summarize the open threads.".into();
    standup["last_run"] = "2026-10-01T09:00:00Z".into();
    standup["next_run"] = "2026-10-01T11:00:00Z".into();
    standup["skipped_unhandled"] = 1.into();
    standup["skipped_no_coordinator"] = 2.into();
    assert_eq!(
        fixture.json(&["routine", "list", "demo", "--json"]),
        json!({
            "schema_version": 1,
            "project": "demo",
            "routine_commands": false,
            "routines": [deps, followup, standup],
            "broken": [{
                "file": "routines/broken.md",
                "error": "a routine must start with a `+++` line"
            }]
        })
    );
    let text = fixture.text(&["routine", "list", "demo"]);
    assert!(
        text.starts_with("deps\tdaily 03:00\tdisabled\tcommand: routine_commands is false, so it does not run\tlast never · next - (disabled)\n"),
        "{text}"
    );
}

#[test]
fn icon_and_tone_are_set_by_cli_and_absent_until_then() {
    let fixture = fixture();
    let project_md = Path::new(&fixture.dir()).join("PROJECT.md");
    let before = std::fs::read_to_string(&project_md).unwrap();
    assert!(
        !before.contains("icon") && !before.contains("tone"),
        "{before}"
    );

    fixture.text(&["set", "demo", "icon", "globe"]);
    fixture.text(&["set", "demo", "tone", "teal"]);
    let listed = fixture.json(&["list", "--json"]);
    assert_eq!(
        (
            &listed["projects"][0]["icon"],
            &listed["projects"][0]["tone"]
        ),
        (&json!("globe"), &json!("teal"))
    );
    assert_eq!(
        fixture.json(&["project", "show", "demo", "--json"])["project"]["icon"],
        "globe"
    );

    let refused = Command::new(BIN)
        .env_clear()
        .env("HOME", fixture.home.path())
        .args(fixture.args(&["set", "demo", "icon", "Globe!"]))
        .output()
        .unwrap();
    assert!(!refused.status.success());
    assert!(refused.stdout.is_empty());

    fixture.text(&["set", "demo", "icon", ""]);
    let after = std::fs::read_to_string(&project_md).unwrap();
    assert!(!after.contains("icon"), "{after}");
    let listed = fixture.json(&["list", "--json"]);
    assert_eq!(
        (
            &listed["projects"][0]["icon"],
            &listed["projects"][0]["tone"]
        ),
        (&json!(""), &json!("teal"))
    );
}

/// Sets a file's modification time, so `sources` is the same on every run.
fn touch(path: &Path, rfc3339: &str) {
    let when: std::time::SystemTime = rfc3339.parse::<jiff::Timestamp>().unwrap().into();
    std::fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(when)
        .unwrap();
}

#[test]
fn project_status_json_is_computed_from_tasks_threads_and_the_inbox() {
    let fixture = fixture();
    write(
        &fixture,
        "TASKS.md",
        "# Tasks\n\n## Backlog\n- [ ] Checkout (codex) · t-0001\n- [ ] Copy fix · t-0002\n- [ ] Write the docs (me)\n- [x] Old work\n",
    );
    let status_text = "# Status\nObjective: Ship the checkout before Friday.\n## Asked\n- [working] Ship it : t-0001\n";
    write(&fixture, "STATUS.md", status_text);
    write(
        &fixture,
        "threads/t-0003.toml",
        "id = \"t-0003\"\ntitle = \"Research\"\nstatus = \"open\"\nkind = \"tab\"\ncreated = \"2026-10-02T09:00:00Z\"\nupdated = \"2026-10-02T09:30:00Z\"\nlast_group = \"working\"\ntask_origin = \"chat\"\n",
    );
    write(&fixture, "threads/t-0003.next.md", "- Pick a library\n");
    let dir = Path::new(&fixture.dir()).to_path_buf();
    touch(&dir.join("TASKS.md"), "2026-10-03T08:00:00Z");
    touch(&dir.join("STATUS.md"), "2026-10-03T09:00:00Z");

    let mut status = fixture.json(&["project", "status", "demo", "--json"]);
    let generated = status["generated_at"].take();
    assert!(
        generated
            .as_str()
            .unwrap()
            .parse::<jiff::Timestamp>()
            .is_ok(),
        "{generated}"
    );
    let checkout_pr = checkout_thread(&fixture)["pr"].clone();
    let no_owner = Value::Null;
    assert_eq!(
        status,
        json!({
            "schema_version": 1,
            "project": "demo",
            "generated_at": null,
            "objective": "Ship the checkout before Friday.",
            "sources": {
                "tasks_updated": "2026-10-03T08:00:00Z",
                "status_file": "STATUS.md",
                "status_updated": "2026-10-03T09:00:00Z",
                "status_chars": status_text.chars().count(),
                "status_limit": 4000,
                "pr_checked": ""
            },
            "items": [
                {
                    "kind": "task", "id": "task:4", "title": "Checkout", "list": "Backlog",
                    "owner": {"kind": "agent", "profile": "codex", "machine": "", "name": ""},
                    "state": "review", "group": "ready-for-review", "task": "Checkout", "thread": "t-0001",
                    "pr": checkout_pr, "created": "2026-10-01T10:00:00Z", "updated": "2026-10-01T10:30:00Z",
                    "flags": []
                },
                {
                    "kind": "task", "id": "task:5", "title": "Copy fix", "list": "Backlog",
                    "owner": {"kind": "unassigned", "profile": "", "machine": "", "name": ""},
                    "state": "done", "group": "resolved", "task": "Copy fix", "thread": "t-0002",
                    "pr": null, "created": "2026-10-01T10:05:00Z", "updated": "2026-10-01T10:50:00Z",
                    "flags": ["thread-resolved", "pr-merged"]
                },
                {
                    "kind": "task", "id": "task:6", "title": "Write the docs", "list": "Backlog",
                    "owner": {"kind": "me", "profile": "", "machine": "", "name": ""},
                    "state": "todo", "group": "", "task": "Write the docs", "thread": "",
                    "pr": null, "created": "", "updated": "", "flags": []
                },
                {
                    "kind": "task", "id": "task:7", "title": "Old work", "list": "Backlog",
                    "owner": {"kind": "unassigned", "profile": "", "machine": "", "name": ""},
                    "state": "done", "group": "", "task": "Old work", "thread": "",
                    "pr": null, "created": "", "updated": "", "flags": ["done-not-deleted"]
                },
                {
                    "kind": "thread", "id": "t-0003", "title": "Research", "list": "",
                    "owner": no_owner, "state": "working", "group": "working", "task": "", "thread": "t-0003",
                    "pr": null, "created": "2026-10-02T09:00:00Z", "updated": "2026-10-02T09:30:00Z",
                    "flags": []
                }
            ],
            "drift": {
                "status_over_limit": 0, "status_stale": 0,
                "thread_resolved": 1, "thread_missing": 0,
                "done_not_deleted": 1, "status_in_owner": 0, "duplicate_title": 0,
                "inbox_unhandled": 1
            },
            "problems": [
                {"rule": "thread-resolved", "line": 5, "task": "Copy fix", "thread": "t-0002",
                 "detail": "the thread is resolved and its PR merged"},
                {"rule": "done-not-deleted", "line": 7, "task": "Old work", "thread": "",
                 "detail": "checked done but still listed"}
            ],
            "next": [{"thread": "t-0003", "line": "Pick a library"}]
        })
    );
}

/// Claude Code's folder for a session run in `cwd`, under `home`.
fn claude_transcript(home: &Path, cwd: &Path, session: &str) -> std::path::PathBuf {
    let folder: String = cwd
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let dir = home.join(".claude/projects").join(folder);
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(format!("{session}.jsonl"))
}

/// `node usage` reads each local node's transcripts (synthetic here) and
/// prints one row per node, the root first; the thread and project documents
/// then carry the same numbers, and a node with nothing read is `null`.
#[test]
fn node_usage_prints_a_row_per_node_and_the_documents_carry_it() {
    let fixture = fixture();
    let dir = std::fs::canonicalize(fixture.dir()).unwrap();
    std::fs::write(
        dir.join(".state/coordinator.json"),
        json!({"agent": "claude", "cwd": dir, "sessions": ["root-1"]}).to_string(),
    )
    .unwrap();
    let row = |id: &str, at: &str, usage: Value| {
        json!({"type": "assistant", "timestamp": at, "requestId": format!("r-{id}"),
            "message": {"id": id, "content": [], "usage": usage}})
        .to_string()
    };
    std::fs::write(
        claude_transcript(fixture.home.path(), &dir, "root-1"),
        [
            row(
                "m1",
                "2026-10-16T10:00:00.250Z",
                json!({"input_tokens": 200, "output_tokens": 1000,
                "cache_read_input_tokens": 1_000_000, "cache_creation_input_tokens": 100}),
            ),
            json!({"type": "system", "subtype": "compact_boundary"}).to_string(),
            String::new(),
        ]
        .join("\n"),
    )
    .unwrap();
    let mut record = std::fs::read_to_string(dir.join("threads/t-0001.toml")).unwrap();
    record.push_str("sessions = [\"0199a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b\"]\n");
    std::fs::write(dir.join("threads/t-0001.toml"), record).unwrap();
    let rollouts = fixture.home.path().join(".codex/sessions/2026/10/16");
    std::fs::create_dir_all(&rollouts).unwrap();
    std::fs::write(
        rollouts.join("rollout-2026-10-16T09-00-00-0199a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b.jsonl"),
        format!(
            "{}\n",
            json!({"timestamp": "2026-10-16T09:30:00Z", "payload": {"type": "token_count", "info": {
                "total_token_usage": {"input_tokens": 5000, "cached_input_tokens": 4000, "output_tokens": 300},
                "last_token_usage": {"input_tokens": 5000, "cached_input_tokens": 4000, "output_tokens": 300}}}})
        ),
    )
    .unwrap();

    let root = json!({"input": 200, "output": 1000, "cache_read": 1_000_000, "cache_write": 100,
        "context": 1_000_300, "peak_context": 1_000_300, "compactions": 1,
        "updated": "2026-10-16T10:00:00Z", "over_cap": []});
    let checkout = json!({"input": 1000, "output": 300, "cache_read": 4000, "cache_write": 0,
        "context": 5000, "peak_context": 5000, "compactions": 0,
        "updated": "2026-10-16T09:30:00Z", "over_cap": []});
    assert_eq!(
        fixture.json(&["node", "usage", "demo", "--json"]),
        json!({"schema_version": 1, "project": "demo", "nodes": [
            {"id": "root", "title": "Coordinator", "usage": root},
            {"id": "t-0001", "title": "Checkout", "usage": checkout},
            {"id": "t-0002", "title": "Copy fix", "usage": null}
        ]})
    );
    let threads = fixture.json(&["thread", "list", "demo", "--json"]);
    assert_eq!(threads["threads"][0]["usage"], checkout);
    assert_eq!(threads["threads"][1]["usage"], Value::Null);
    assert_eq!(
        fixture.json(&["list", "--json"])["projects"][0]["usage"],
        root
    );
}

/// `node move` as clients call it: `--parent <id>` or `--parent=<id>`, one
/// JSON line with `--json`, and a refusal that exits non-zero with its reason
/// on standard error and nothing on standard output. A node with another
/// parent to go to lists `move` among its actions.
#[test]
fn node_move_prints_its_document_and_refuses_on_standard_error() {
    let fixture = fixture();
    let dir = Path::new(&fixture.root).join("demo");
    std::fs::write(
        dir.join("threads/t-0003.toml"),
        "id = \"t-0003\"\ntitle = \"Research\"\nstatus = \"open\"\nkind = \"tab\"\n",
    )
    .unwrap();

    let help = Command::new(BIN)
        .env_clear()
        .env("HOME", fixture.home.path())
        .args(["node", "move", "--help"])
        .output()
        .unwrap();
    assert!(help.status.success());

    let actions = |id: &str| {
        let listed = fixture.json(&["node", "list", "demo", "--json"]);
        listed["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|node| node["id"] == id)
            .unwrap()["actions"]
            .clone()
    };
    assert_eq!(actions("t-0003"), json!(["move"]));
    // Root is where it is and its only child is a worker: nowhere else to go.
    assert_eq!(actions("t-0001"), json!([]));

    assert_eq!(
        fixture.json(&[
            "node",
            "move",
            "demo",
            "t-0003",
            "--parent=t-0001",
            "--json"
        ]),
        json!({"schema_version": 1, "id": "t-0003", "parent_id": "t-0001"})
    );
    assert_eq!(
        fixture.json(&["node", "show", "demo", "t-0003", "--json"])["thread"]["parent_id"],
        "t-0001"
    );
    assert_eq!(actions("t-0003"), json!(["move"]));
    assert_eq!(
        fixture.json(&[
            "node", "move", "demo", "t-0003", "--parent", "root", "--json"
        ]),
        json!({"schema_version": 1, "id": "t-0003", "parent_id": "root"})
    );
    assert!(
        fixture
            .text(&["node", "move", "demo", "t-0003", "--parent", "t-0001"])
            .contains("t-0003")
    );

    let refused = Command::new(BIN)
        .env_clear()
        .env("HOME", fixture.home.path())
        .args(fixture.args(&[
            "node", "move", "demo", "t-0003", "--parent", "t-0002", "--json",
        ]))
        .output()
        .unwrap();
    assert!(!refused.status.success());
    assert!(refused.stdout.is_empty());
    let reason = String::from_utf8_lossy(&refused.stderr);
    assert!(
        reason.contains("worker node `t-0002` cannot spawn children"),
        "{reason}"
    );
    assert_eq!(
        fixture.json(&["node", "show", "demo", "t-0003", "--json"])["thread"]["parent_id"],
        "t-0001"
    );
}

/// A project bound to a session with no root coordinator (its record names
/// no pane) has no `coordinator` object, as a project never opened, and
/// `context` reads it as any other project.
#[test]
fn a_project_without_a_root_coordinator_has_no_coordinator_object() {
    let fixture = fixture();
    let dir = Path::new(&fixture.root).join("demo");
    let opened = |pane: &str| {
        std::fs::write(
            dir.join(".state/coordinator.json"),
            json!({
                "socket": "/nonexistent/herdr.sock",
                "workspace_id": "w7",
                "tab_id": "w7:t1",
                "pane_id": pane,
                "cwd": std::fs::canonicalize(&dir).unwrap(),
            })
            .to_string(),
        )
        .unwrap();
        fixture.json(&["project", "show", "demo", "--json"])["project"].clone()
    };
    let organizer = opened("");
    assert!(organizer.get("coordinator").is_none(), "{organizer}");
    assert_eq!(opened("w7:p1")["coordinator"]["pane_id"], "w7:p1");
    opened("");
    assert!(
        fixture
            .text(&["context", "demo", "--peek"])
            .contains("Project: demo (active)")
    );
}

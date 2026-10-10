//! `thread set` and `thread merge` against the built binary, with a fake `gh`
//! first on `PATH` that logs its arguments. Nothing reaches GitHub.

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Output};

const BIN: &str = env!("CARGO_BIN_EXE_herdr-organizations");
const HEAD: &str = "0123456789abcdef0123456789abcdef01234567";
const URL: &str = "https://github.com/owner/app/pull/7";

struct Fixture {
    home: tempfile::TempDir,
    root: PathBuf,
}

impl Fixture {
    fn new() -> Fixture {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join("root");
        let fixture = Fixture { home, root };
        let bin = fixture.home.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        let gh = bin.join("gh");
        // Logs every call; `pr view` serves view.json, `pr merge` succeeds.
        std::fs::write(
            &gh,
            format!(
                "#!/bin/sh\necho \"$*\" >> '{log}'\ncase \"$2\" in view) cat '{view}' ;; esac\n",
                log = fixture.log().display(),
                view = fixture.view_path().display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(fixture.run(&["new", "Demo"]).status.success());
        std::fs::write(
            fixture.root.join("demo/threads/t-0001.toml"),
            format!(
                "id = \"t-0001\"\ntitle = \"Checkout\"\nstatus = \"open\"\nbranch = \"hp/demo/t-0001-checkout\"\norigin = \"git@github.com:owner/app.git\"\npr = \"{URL}\"\npr_state = \"OPEN\"\n"
            ),
        )
        .unwrap();
        fixture
    }

    fn log(&self) -> PathBuf {
        self.home.path().join("gh.log")
    }

    fn view_path(&self) -> PathBuf {
        self.home.path().join("view.json")
    }

    fn serve(&self, review: &str, checks: &str) {
        std::fs::write(
            self.view_path(),
            format!(
                r#"{{"state":"OPEN","reviewDecision":"{review}","headRefName":"hp/demo/t-0001-checkout","headRefOid":"{HEAD}","headRepository":{{"name":"app"}},"headRepositoryOwner":{{"login":"owner"}},"mergeable":"MERGEABLE","statusCheckRollup":[{checks}]}}"#
            ),
        )
        .unwrap();
    }

    fn run(&self, args: &[&str]) -> Output {
        self.run_in_pane("", args)
    }

    /// As if run from the Herdr pane `pane`; empty means a plain terminal.
    fn run_in_pane(&self, pane: &str, args: &[&str]) -> Output {
        let path = format!("{}:/usr/bin:/bin", self.home.path().join("bin").display());
        let mut cmd = Command::new(BIN);
        cmd.env_clear()
            .env("HOME", self.home.path())
            .env("PATH", path);
        if !pane.is_empty() {
            cmd.env("HERDR_PANE_ID", pane);
        }
        cmd.arg("--root")
            .arg(&self.root)
            .args(args)
            .output()
            .unwrap()
    }

    fn gh_calls(&self) -> Vec<String> {
        std::fs::read_to_string(self.log())
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn record(&self) -> String {
        std::fs::read_to_string(self.root.join("demo/threads/t-0001.toml")).unwrap()
    }

    fn inbox(&self) -> String {
        std::fs::read_dir(self.root.join("demo/inbox"))
            .unwrap()
            .flatten()
            .filter_map(|entry| std::fs::read_to_string(entry.path()).ok())
            .collect()
    }
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn thread_set_turns_flags_on_and_off_and_needs_at_least_one() {
    let fixture = Fixture::new();
    let out = fixture.run(&[
        "thread",
        "set",
        "demo",
        "t-0001",
        "--auto-merge",
        "on",
        "--auto-fix-ci",
        "on",
        "--json",
    ]);
    assert!(out.status.success(), "{}", stderr(&out));
    let printed: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        printed,
        serde_json::json!({"schema_version": 1, "id": "t-0001", "auto_fix_ci": true, "auto_merge": true})
    );
    assert!(fixture.record().contains("auto_merge = true"));

    let out = fixture.run(&["thread", "set", "demo", "t-0001", "--auto-merge", "off"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "t-0001: auto-fix-ci on, auto-merge off\n"
    );
    assert!(fixture.record().contains("auto_merge = false"));

    assert!(
        !fixture
            .run(&["thread", "set", "demo", "t-0001"])
            .status
            .success()
    );
    assert!(fixture.gh_calls().is_empty(), "set never calls gh");
}

#[test]
fn thread_merge_refuses_without_approval_or_with_unfinished_checks() {
    let fixture = Fixture::new();
    fixture.serve(
        "REVIEW_REQUIRED",
        r#"{"name":"build","conclusion":"SUCCESS"}"#,
    );
    let out = fixture.run(&["thread", "merge", "demo", "t-0001"]);
    assert!(!out.status.success());
    assert!(stderr(&out).contains("not approved"), "{}", stderr(&out));

    fixture.serve(
        "APPROVED",
        r#"{"name":"build","conclusion":"SUCCESS"},{"name":"e2e","status":"QUEUED"}"#,
    );
    let out = fixture.run(&["thread", "merge", "demo", "t-0001"]);
    assert!(!out.status.success());
    assert!(stderr(&out).contains("not finished"), "{}", stderr(&out));

    assert!(
        fixture
            .gh_calls()
            .iter()
            .all(|call| !call.starts_with("pr merge")),
        "{:?}",
        fixture.gh_calls()
    );
    assert!(fixture.record().contains("pr_state = \"OPEN\""));
}

#[test]
fn thread_merge_merges_the_checked_commit_and_leaves_an_inbox_item() {
    let fixture = Fixture::new();
    fixture.serve("APPROVED", r#"{"name":"build","conclusion":"SUCCESS"}"#);
    let out = fixture.run(&["thread", "merge", "demo", "t-0001", "--json"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let printed: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        printed,
        serde_json::json!({"schema_version": 1, "id": "t-0001", "pr": URL, "merged": true})
    );
    let calls = fixture.gh_calls();
    assert_eq!(calls.len(), 2, "{calls:?}");
    assert!(calls[0].starts_with("pr view --json "), "{calls:?}");
    assert_eq!(
        calls[1],
        format!("pr merge --squash --match-head-commit {HEAD} -- {URL}")
    );
    assert!(fixture.record().contains("pr_state = \"MERGED\""));
    assert!(fixture.inbox().contains("was merged with `thread merge`"));
}

#[test]
fn thread_merge_without_a_pull_request_never_calls_gh() {
    let fixture = Fixture::new();
    let record = fixture.record().replace(&format!("pr = \"{URL}\"\n"), "");
    std::fs::write(fixture.root.join("demo/threads/t-0001.toml"), record).unwrap();
    let out = fixture.run(&["thread", "merge", "demo", "t-0001"]);
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("has no pull request"),
        "{}",
        stderr(&out)
    );
    assert!(fixture.gh_calls().is_empty());
}

#[test]
fn an_agent_pane_of_the_project_cannot_merge_or_turn_auto_merge_on() {
    let fixture = Fixture::new();
    let record = fixture.record() + "pane_id = \"w2:p1\"\n";
    std::fs::write(fixture.root.join("demo/threads/t-0001.toml"), record).unwrap();
    fixture.serve("APPROVED", r#"{"name":"build","conclusion":"SUCCESS"}"#);

    let out = fixture.run_in_pane(
        "w2:p1",
        &["thread", "set", "demo", "t-0001", "--auto-merge", "on"],
    );
    assert!(!out.status.success());
    assert!(stderr(&out).contains("agent pane"), "{}", stderr(&out));
    assert!(
        fixture.record().contains("auto_merge = false")
            || !fixture.record().contains("auto_merge = true")
    );

    let out = fixture.run_in_pane("w2:p1", &["thread", "merge", "demo", "t-0001"]);
    assert!(!out.status.success());
    assert!(stderr(&out).contains("agent pane"), "{}", stderr(&out));
    assert!(fixture.gh_calls().is_empty(), "{:?}", fixture.gh_calls());

    let out = fixture.run_in_pane(
        "w2:p1",
        &[
            "thread",
            "set",
            "demo",
            "t-0001",
            "--auto-merge",
            "off",
            "--auto-fix-ci",
            "on",
        ],
    );
    assert!(
        out.status.success(),
        "turning it off stays allowed: {}",
        stderr(&out)
    );

    let out = fixture.run_in_pane(
        "w9:p9",
        &["thread", "set", "demo", "t-0001", "--auto-merge", "on"],
    );
    assert!(
        out.status.success(),
        "a pane the project did not record: {}",
        stderr(&out)
    );
}

#[test]
fn after_thread_merge_the_json_reports_a_blocker_until_the_ticker_reads_again() {
    let fixture = Fixture::new();
    fixture.serve("APPROVED", r#"{"name":"build","conclusion":"SUCCESS"}"#);
    // The ticker's cached summary from before the merge: open, approved, green.
    std::fs::create_dir_all(fixture.root.join("demo/.state")).unwrap();
    std::fs::write(
        fixture.root.join("demo/.state/ticker.json"),
        format!(
            r#"{{"prs":{{"t-0001":{{"state":"OPEN","review_decision":"APPROVED","checks":{{"passed":1}},"mergeable":"MERGEABLE","head_oid":"{HEAD}"}}}}}}"#
        ),
    )
    .unwrap();
    let blocker = || {
        let out = fixture.run(&["thread", "list", "demo", "--json"]);
        assert!(out.status.success(), "{}", stderr(&out));
        let list: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        list["threads"][0]["pr"]["merge_blocker"].clone()
    };
    assert_eq!(blocker(), serde_json::Value::Null);

    let out = fixture.run(&["thread", "merge", "demo", "t-0001"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(blocker(), "it is not open (state MERGED)");
}

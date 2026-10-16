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
        // Logs every call; `pr view` serves view.json, `pr merge` succeeds
        // unless a push was simulated (the `moved` file), as GitHub refuses a
        // merge pinned to a head commit that is no longer the head.
        std::fs::write(
            &gh,
            format!(
                "#!/bin/sh\necho \"$*\" >> '{log}'\ncase \"$2\" in\n  view) cat '{view}' ;;\n  merge) if [ -e '{moved}' ]; then echo 'Head branch was modified. Review and try the merge again.' >&2; exit 1; fi ;;\nesac\n",
                log = fixture.log().display(),
                view = fixture.view_path().display(),
                moved = fixture.moved().display()
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

    fn moved(&self) -> PathBuf {
        self.home.path().join("moved")
    }

    fn serve(&self, review: &str, checks: &str) {
        std::fs::write(
            self.view_path(),
            format!(
                r#"{{"state":"OPEN","reviewDecision":"{review}","headRefName":"hp/demo/t-0001-checkout","headRefOid":"{HEAD}","headRepository":{{"name":"app"}},"headRepositoryOwner":{{"login":"owner"}},"mergeable":"MERGEABLE","baseRefName":"main","statusCheckRollup":[{checks}]}}"#
            ),
        )
        .unwrap();
    }

    /// The project's own safety table in config.toml, keyed by its canonical
    /// path as `safety set` writes it.
    fn configure(&self, lines: &str) {
        let dir = self.home.path().join(".config/herdr-projects");
        std::fs::create_dir_all(&dir).unwrap();
        let project = std::fs::canonicalize(self.root.join("demo")).unwrap();
        std::fs::write(
            dir.join("config.toml"),
            format!("[safety.\"{}\"]\n{lines}", project.display()),
        )
        .unwrap();
    }

    fn config(&self) -> String {
        std::fs::read_to_string(self.home.path().join(".config/herdr-projects/config.toml"))
            .unwrap_or_default()
    }

    /// A merge gate script with `body`, configured as the project's
    /// `merge_gate` with one argument; `more` adds config lines.
    fn gate(&self, body: &str, more: &str) {
        let gate = self.home.path().join("gate.sh");
        std::fs::write(&gate, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&gate, std::fs::Permissions::from_mode(0o755)).unwrap();
        self.configure(&format!(
            "merge_gate = [\"{}\", \"--strict\"]\n{more}",
            gate.display()
        ));
    }

    /// What the gate wrote to `gate.log`, empty when it never ran.
    fn gate_log(&self) -> String {
        std::fs::read_to_string(self.home.path().join("gate.log")).unwrap_or_default()
    }

    /// The ticker's cached summary of the served pull request.
    fn cache_summary(&self, review: &str) {
        std::fs::create_dir_all(self.root.join("demo/.state")).unwrap();
        std::fs::write(
            self.root.join("demo/.state/ticker.json"),
            format!(
                r#"{{"prs":{{"t-0001":{{"state":"OPEN","review_decision":"{review}","mergeable":"MERGEABLE","base_ref":"main","head_oid":"{HEAD}"}}}}}}"#
            ),
        )
        .unwrap();
    }

    fn thread_json(&self) -> serde_json::Value {
        let out = self.run(&["thread", "show", "demo", "t-0001", "--json"]);
        assert!(out.status.success(), "{}", stderr(&out));
        printed(&out)["thread"].clone()
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

fn printed(out: &Output) -> serde_json::Value {
    serde_json::from_slice(&out.stdout).unwrap_or_else(|error| {
        panic!(
            "{error}: {}{}",
            String::from_utf8_lossy(&out.stdout),
            stderr(out)
        )
    })
}

#[test]
fn pr_action_refuses_what_the_pull_request_does_not_call_for_and_says_why() {
    let fixture = Fixture::new();
    fixture.serve("APPROVED", r#"{"name":"build","conclusion":"SUCCESS"}"#);
    let out = fixture.run(&["thread", "pr-action", "demo", "t-0001", "fix-ci", "--json"]);
    assert!(!out.status.success());
    let reason = "no check fails on its pull request (1 passed, 0 pending)";
    assert_eq!(
        printed(&out),
        serde_json::json!({
            "schema_version": 1, "ok": false, "id": "t-0001", "action": "fix-ci",
            "pr": URL, "sent": false, "message": "", "reason": reason
        })
    );
    assert!(stderr(&out).contains(reason), "{}", stderr(&out));

    for (action, why) in [
        (
            "address-comments",
            "no comments and no review asking for changes",
        ),
        ("resolve-conflicts", "no merge conflict"),
    ] {
        let out = fixture.run(&["thread", "pr-action", "demo", "t-0001", action, "--json"]);
        assert!(!out.status.success(), "{action}");
        let document = printed(&out);
        assert_eq!(
            (document["ok"].clone(), document["sent"].clone()),
            (false.into(), false.into())
        );
        assert!(
            document["reason"].as_str().unwrap().contains(why),
            "{action}: {document}"
        );
    }

    let out = fixture.run(&["thread", "pr-action", "demo", "t-0001", "fix-ci"]);
    assert!(!out.status.success());
    assert!(
        out.stdout.is_empty(),
        "text mode prints the refusal on stderr only"
    );
    assert!(stderr(&out).contains(reason), "{}", stderr(&out));

    let out = fixture.run(&["thread", "pr-action", "demo", "t-0001", "merge"]);
    assert!(
        !out.status.success(),
        "merging is `thread merge`, not a message"
    );

    let calls = fixture.gh_calls();
    assert!(
        calls.iter().all(|call| call.starts_with("pr view --json ")),
        "{calls:?}"
    );
    assert!(
        !fixture.inbox().contains("pr-action"),
        "a refusal leaves no item"
    );
}

#[test]
fn pr_action_and_pr_create_refuse_without_what_they_need_and_never_reach_github() {
    let fixture = Fixture::new();
    let out = fixture.run(&["thread", "pr-create", "demo", "t-0001", "--json"]);
    assert!(!out.status.success());
    assert_eq!(
        printed(&out),
        serde_json::json!({
            "schema_version": 1, "ok": false, "id": "t-0001", "pr": URL,
            "reason": format!("it already has a pull request: {URL}")
        })
    );

    let record = fixture.record().replace(&format!("pr = \"{URL}\"\n"), "");
    let path = fixture.root.join("demo/threads/t-0001.toml");
    std::fs::write(&path, &record).unwrap();
    let out = fixture.run(&["thread", "pr-action", "demo", "t-0001", "fix-ci", "--json"]);
    assert!(!out.status.success());
    let document = printed(&out);
    assert_eq!(document["pr"], "");
    assert!(
        document["reason"]
            .as_str()
            .unwrap()
            .contains("has no pull request"),
        "{document}"
    );

    let reason_of = |extra: &str| {
        std::fs::write(&path, format!("{record}{extra}")).unwrap();
        let out = fixture.run(&["thread", "pr-create", "demo", "t-0001", "--json"]);
        assert!(!out.status.success(), "{extra}");
        printed(&out)["reason"].as_str().unwrap().to_string()
    };
    assert!(reason_of("kind = \"tab\"\n").contains("worktree thread"));
    assert!(reason_of("").contains("no report"));
    assert!(
        reason_of("report_hash = \"h\"\n").contains("did not answer"),
        "the project was never opened, so its agent's state is unknown"
    );
    assert!(fixture.gh_calls().is_empty(), "{:?}", fixture.gh_calls());
}

#[test]
fn without_merge_settings_github_decides_and_the_merge_is_a_squash() {
    let fixture = Fixture::new();
    fixture.serve("APPROVED", "");
    let out = fixture.run(&["thread", "merge", "demo", "t-0001"]);
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("no checks are reported"),
        "{}",
        stderr(&out)
    );

    // A safety table that sets none of the merge keys changes nothing.
    fixture.configure("yolo = false\n");
    fixture.serve(
        "REVIEW_REQUIRED",
        r#"{"name":"build","conclusion":"SUCCESS"}"#,
    );
    let out = fixture.run(&["thread", "merge", "demo", "t-0001"]);
    assert!(stderr(&out).contains("not approved"), "{}", stderr(&out));
    fixture.serve("APPROVED", r#"{"name":"build","conclusion":"SUCCESS"}"#);
    let out = fixture.run(&["thread", "merge", "demo", "t-0001"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let calls = fixture.gh_calls();
    assert_eq!(
        calls.last().unwrap(),
        &format!("pr merge --squash --match-head-commit {HEAD} -- {URL}")
    );
    assert_eq!(
        calls.iter().filter(|c| c.starts_with("pr merge")).count(),
        1,
        "{calls:?}"
    );
}

#[test]
fn a_passing_merge_gate_merges_without_approval_or_checks_with_its_method_pinned_to_the_head_it_saw()
 {
    let fixture = Fixture::new();
    let repo = fixture.home.path().join("app");
    let worktree = fixture.home.path().join("wt");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(&worktree).unwrap();
    let record = fixture.record()
        + &format!(
            "repo = \"{}\"\nworktree_path = \"{}\"\n",
            repo.display(),
            worktree.display()
        );
    std::fs::write(fixture.root.join("demo/threads/t-0001.toml"), record).unwrap();
    let log = fixture.home.path().join("gate.log");
    fixture.gate(
        &format!(
            "{{ env | grep '^HP_' | sort; echo \"args=$*\"; echo \"cwd=$(pwd -P)\"; }} > '{}'\nexit 0",
            log.display()
        ),
        "merge_method = \"rebase\"\n",
    );
    fixture.serve("REVIEW_REQUIRED", "");

    let out = fixture.run(&["thread", "merge", "demo", "t-0001", "--json"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        printed(&out),
        serde_json::json!({"schema_version": 1, "id": "t-0001", "pr": URL, "merged": true})
    );
    let calls = fixture.gh_calls();
    assert_eq!(calls.len(), 2, "{calls:?}");
    assert_eq!(
        calls[1],
        format!("pr merge --rebase --match-head-commit {HEAD} -- {URL}")
    );
    let seen = fixture.gate_log();
    for line in [
        "HP_PROJECT=demo".to_string(),
        "HP_THREAD_ID=t-0001".into(),
        format!("HP_PR_URL={URL}"),
        "HP_PR_NUMBER=7".into(),
        "HP_REPO=owner/app".into(),
        format!("HP_HEAD_SHA={HEAD}"),
        "HP_HEAD_REF=hp/demo/t-0001-checkout".into(),
        "HP_BASE_REF=main".into(),
        format!(
            "HP_WORKTREE={}",
            std::fs::canonicalize(&worktree).unwrap().display()
        ),
        "args=--strict".into(),
        format!("cwd={}", std::fs::canonicalize(&repo).unwrap().display()),
    ] {
        assert!(seen.lines().any(|l| l == line), "{line} not in:\n{seen}");
    }
    assert!(fixture.record().contains("pr_state = \"MERGED\""));

    // `--method` still wins over the project's `merge_method`.
    let reopened = fixture
        .record()
        .replace("pr_state = \"MERGED\"", "pr_state = \"OPEN\"");
    std::fs::write(fixture.root.join("demo/threads/t-0001.toml"), reopened).unwrap();
    let out = fixture.run(&["thread", "merge", "demo", "t-0001", "--method", "merge"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        fixture.gh_calls().last().unwrap(),
        &format!("pr merge --merge --match-head-commit {HEAD} -- {URL}")
    );
}

#[test]
fn a_refusing_merge_gate_blocks_the_merge_and_its_reason_reaches_the_inbox_and_json_once() {
    let fixture = Fixture::new();
    fixture.gate(
        "printf 'waiting for the reviewer \\033[31mPASS\\nsecond line\\n'\nexit 1",
        "",
    );
    fixture.serve("REVIEW_REQUIRED", "");
    fixture.cache_summary("REVIEW_REQUIRED");
    let before = fixture.thread_json();
    assert_eq!(
        before["pr"]["merge_blocker"],
        serde_json::Value::Null,
        "the gate has not run on this commit: `thread merge` would run it"
    );
    assert_eq!(before["pr"]["actions"][0], "merge");

    let reason = "waiting for the reviewer [31mPASS";
    for _ in 0..2 {
        let out = fixture.run(&["thread", "merge", "demo", "t-0001"]);
        assert!(!out.status.success());
        assert!(stderr(&out).contains(reason), "{}", stderr(&out));
        assert!(!stderr(&out).contains("second line"), "{}", stderr(&out));
    }
    assert!(
        fixture
            .gh_calls()
            .iter()
            .all(|c| !c.starts_with("pr merge")),
        "{:?}",
        fixture.gh_calls()
    );
    assert!(fixture.record().contains("pr_state = \"OPEN\""));
    let inbox = fixture.inbox();
    assert_eq!(
        inbox.matches(reason).count(),
        1,
        "one item per reason:\n{inbox}"
    );
    assert!(inbox.contains("merge gate refused"), "{inbox}");
    assert!(!inbox.contains('\u{1b}'), "{inbox:?}");
    let after = fixture.thread_json();
    assert_eq!(after["pr"]["merge_blocker"], reason);
    assert_eq!(
        after["pr"]["actions"][0], "merge",
        "a merge asks the gate again: its verdict can change on the same commit"
    );

    // A silent refusal is named by its exit code, and a new reason is news.
    fixture.gate("exit 3", "");
    let out = fixture.run(&["thread", "merge", "demo", "t-0001"]);
    assert!(
        stderr(&out).contains("merge gate exited 3"),
        "{}",
        stderr(&out)
    );
    assert_eq!(
        fixture
            .inbox()
            .matches("event = \"merge gate refused\"")
            .count(),
        2
    );
    assert_eq!(
        fixture.thread_json()["pr"]["merge_blocker"],
        "merge gate exited 3"
    );
}

#[test]
fn a_merge_gate_that_runs_too_long_is_a_refusal() {
    let fixture = Fixture::new();
    fixture.gate("sleep 5\nexit 0", "merge_gate_timeout = 1\n");
    fixture.serve("REVIEW_REQUIRED", "");
    let out = fixture.run(&["thread", "merge", "demo", "t-0001"]);
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("merge gate timed out"),
        "{}",
        stderr(&out)
    );
    assert!(
        fixture
            .gh_calls()
            .iter()
            .all(|c| !c.starts_with("pr merge"))
    );
}

#[test]
fn a_push_between_the_gate_and_the_merge_is_refused_by_github_and_not_recorded_as_merged() {
    let fixture = Fixture::new();
    let log = fixture.home.path().join("gate.log");
    // The gate passes, and a push lands while it runs.
    fixture.gate(
        &format!(
            "echo \"$HP_HEAD_SHA\" > '{}'\ntouch '{}'\nexit 0",
            log.display(),
            fixture.moved().display()
        ),
        "",
    );
    fixture.serve("REVIEW_REQUIRED", "");
    let out = fixture.run(&["thread", "merge", "demo", "t-0001"]);
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("Head branch was modified"),
        "{}",
        stderr(&out)
    );
    assert_eq!(fixture.gate_log().trim(), HEAD);
    assert_eq!(
        fixture.gh_calls().last().unwrap(),
        &format!("pr merge --squash --match-head-commit {HEAD} -- {URL}")
    );
    assert!(fixture.record().contains("pr_state = \"OPEN\""));
    assert!(!fixture.inbox().contains("was merged"));
}

#[test]
fn conflicts_drafts_and_failed_or_pending_checks_block_even_when_the_gate_would_pass() {
    let fixture = Fixture::new();
    let log = fixture.home.path().join("gate.log");
    fixture.gate(&format!("echo ran >> '{}'\nexit 0", log.display()), "");
    let cases = [
        (
            "",
            r#""state":"OPEN""#,
            r#""isDraft":true,"state":"OPEN""#,
            "it is a draft",
        ),
        ("", "MERGEABLE", "CONFLICTING", "merge conflicts"),
        (
            r#"{"name":"build","conclusion":"FAILURE"}"#,
            "",
            "",
            "1 check(s) failed",
        ),
        (
            r#"{"name":"e2e","status":"QUEUED"}"#,
            "",
            "",
            "have not finished",
        ),
        (
            "",
            r#""state":"OPEN""#,
            r#""state":"CLOSED""#,
            "it is not open",
        ),
    ];
    for (checks, from, to, why) in cases {
        fixture.serve("REVIEW_REQUIRED", checks);
        let view = std::fs::read_to_string(fixture.view_path()).unwrap();
        std::fs::write(fixture.view_path(), view.replace(from, to)).unwrap();
        let out = fixture.run(&["thread", "merge", "demo", "t-0001"]);
        assert!(!out.status.success(), "{why}");
        assert!(stderr(&out).contains(why), "{why}: {}", stderr(&out));
    }
    assert_eq!(
        fixture.gate_log(),
        "",
        "the gate never runs on a blocked pull request"
    );
    assert!(
        fixture
            .gh_calls()
            .iter()
            .all(|c| !c.starts_with("pr merge"))
    );
}

#[test]
fn an_agent_pane_cannot_merge_through_the_gate_or_change_the_merge_settings() {
    let fixture = Fixture::new();
    let record = fixture.record() + "pane_id = \"w2:p1\"\n";
    std::fs::write(fixture.root.join("demo/threads/t-0001.toml"), record).unwrap();
    let log = fixture.home.path().join("gate.log");
    fixture.gate(&format!("echo ran >> '{}'\nexit 0", log.display()), "");
    fixture.serve("REVIEW_REQUIRED", "");
    let config = fixture.config();

    let out = fixture.run_in_pane("w2:p1", &["thread", "merge", "demo", "t-0001"]);
    assert!(!out.status.success());
    assert!(stderr(&out).contains("agent pane"), "{}", stderr(&out));
    assert_eq!(fixture.gate_log(), "");
    assert!(fixture.gh_calls().is_empty(), "{:?}", fixture.gh_calls());

    for (key, value) in [
        ("merge_gate", "/opt/gates/approve.sh"),
        ("merge_gate_timeout", "5"),
        ("merge_method", "merge"),
        ("auto_merge", "on"),
        // `default` removes the key, which can turn a project's opt-out off.
        ("merge_gate", "default"),
        ("merge_gate_timeout", "default"),
        ("merge_method", "default"),
        ("auto_merge", "default"),
    ] {
        for pane in ["w2:p1", ""] {
            let out = fixture.run_in_pane(pane, &["safety", "set", "demo", key, value]);
            assert!(!out.status.success(), "{key} from {pane:?}");
            assert!(stderr(&out).contains("by hand"), "{key}: {}", stderr(&out));
        }
    }
    assert_eq!(fixture.config(), config);
}

#[test]
fn context_and_project_show_name_the_gate_program_method_and_auto_merge() {
    let fixture = Fixture::new();
    let shown = |key: &str| {
        let out = fixture.run(&["project", "show", "demo", "--json"]);
        assert!(out.status.success(), "{}", stderr(&out));
        printed(&out)["project"][key].clone()
    };
    assert_eq!(
        (
            shown("merge_gate"),
            shown("merge_method"),
            shown("auto_merge")
        ),
        ("".into(), "squash".into(), false.into())
    );
    fixture.gate("exit 0", "merge_method = \"merge\"\nauto_merge = true\n");
    let gate = fixture.home.path().join("gate.sh");
    assert_eq!(shown("merge_gate"), gate.display().to_string());
    assert_eq!(shown("merge_method"), "merge");
    assert_eq!(shown("auto_merge"), true);

    let out = fixture.run(&["context", "demo", "--peek"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let context = String::from_utf8_lossy(&out.stdout);
    let safety = context
        .lines()
        .find(|l| l.starts_with("Safety:"))
        .unwrap_or_default();
    assert!(
        safety.ends_with(&format!(
            "merge_gate={} merge_method=merge auto_merge=on",
            gate.display()
        )),
        "{safety}"
    );
    assert!(!safety.contains("--strict"), "only the program: {safety}");
}

#[test]
fn a_relative_merge_gate_is_a_config_error_and_nothing_runs() {
    let fixture = Fixture::new();
    let repo = fixture.home.path().join("app");
    std::fs::create_dir_all(&repo).unwrap();
    let record = fixture.record() + &format!("repo = \"{}\"\n", repo.display());
    std::fs::write(fixture.root.join("demo/threads/t-0001.toml"), record).unwrap();
    // An agent's script in the thread's repository, under the gate's name.
    let ran = fixture.home.path().join("agent-script-ran");
    let script = repo.join("approve.sh");
    std::fs::write(
        &script,
        format!("#!/bin/sh\ntouch '{}'\nexit 0\n", ran.display()),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    fixture.configure("merge_gate = [\"./approve.sh\"]\n");
    fixture.serve("REVIEW_REQUIRED", "");

    let out = fixture.run(&["thread", "merge", "demo", "t-0001"]);
    assert!(!out.status.success());
    assert!(stderr(&out).contains("absolute"), "{}", stderr(&out));
    assert!(!ran.exists(), "the repository's script ran");
    assert!(fixture.gh_calls().is_empty(), "{:?}", fixture.gh_calls());
}

#[test]
fn an_unreadable_config_fails_the_merge_instead_of_skipping_the_gate() {
    let fixture = Fixture::new();
    fixture.gate("exit 1", "");
    // Valid TOML, then one byte that is not UTF-8 in a comment.
    let path = fixture
        .home
        .path()
        .join(".config/herdr-projects/config.toml");
    let mut bytes = std::fs::read(&path).unwrap();
    bytes.extend_from_slice(b"# caf\xe9\n");
    std::fs::write(&path, bytes).unwrap();
    fixture.serve("APPROVED", r#"{"name":"build","conclusion":"SUCCESS"}"#);

    let out = fixture.run(&["thread", "merge", "demo", "t-0001"]);
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("could not be read"),
        "{}",
        stderr(&out)
    );
    assert!(
        fixture
            .gh_calls()
            .iter()
            .all(|c| !c.starts_with("pr merge")),
        "{:?}",
        fixture.gh_calls()
    );
}

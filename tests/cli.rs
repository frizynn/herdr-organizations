//! End-to-end checks of the built binary with a scrubbed environment.

use std::path::Path;
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_herdr-organizations");
const LEGACY_BIN: &str = env!("CARGO_BIN_EXE_herdr-projects");

fn hp(home: &Path, args: &[&str]) -> std::process::Output {
    Command::new(BIN)
        .env_clear()
        .env("HOME", home)
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn context_prints_a_usable_prefix_in_a_scrubbed_environment() {
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join("my root");
    let root_arg = root.to_str().unwrap();
    assert!(
        hp(home.path(), &["--root", root_arg, "new", "Demo"])
            .status
            .success()
    );

    let out = hp(
        home.path(),
        &["--root", root_arg, "context", "demo", "--peek"],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8(out.stdout).unwrap();
    let prefix = text
        .lines()
        .next()
        .unwrap()
        .strip_prefix("Commands: ")
        .unwrap();
    // Fixed shape `<binary> --root <root>`, with the spaced root shell-quoted.
    assert_eq!(prefix, format!("{BIN} --root '{root_arg}'"));

    // The printed prefix works as typed, from a bare shell.
    let listed = Command::new("/bin/sh")
        .env_clear()
        .env("HOME", home.path())
        .args(["-c", &format!("{prefix} list")])
        .output()
        .unwrap();
    assert!(listed.status.success());
    assert_eq!(
        String::from_utf8_lossy(&listed.stdout),
        "demo\tactive\tno threads\n"
    );
}

#[test]
fn legacy_thread_adopt_cli_arguments_remain_available() {
    let home = tempfile::tempdir().unwrap();
    let output = hp(home.path(), &["thread", "adopt", "--help"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let help = String::from_utf8_lossy(&output.stdout);
    assert!(help.contains("thread adopt"), "{help}");
    assert!(help.contains("--pane"), "{help}");
    assert!(help.contains("--title <TITLE>"), "{help}");
}

#[test]
fn node_resolve_exposes_the_non_destructive_close_view_option() {
    let home = tempfile::tempdir().unwrap();
    let output = hp(home.path(), &["node", "resolve", "--help"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let help = String::from_utf8_lossy(&output.stdout);
    assert!(help.contains("--close-view"), "{help}");
    assert!(help.contains("surface, keeping Git artifacts"), "{help}");
}

#[test]
fn peek_records_nothing_and_context_records_seen_items() {
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join("root");
    let root_arg = root.to_str().unwrap();
    assert!(
        hp(home.path(), &["--root", root_arg, "new", "demo"])
            .status
            .success()
    );
    let item = "+++\nid = \"20260917T000000Z-routine-r-1\"\nkind = \"routine\"\nsubject = \"r\"\ncreated = \"x\"\nsummary = \"s\"\n+++\n";
    std::fs::write(
        root.join("demo/inbox/20260917T000000Z-routine-r-1.md"),
        item,
    )
    .unwrap();
    let seen = root.join("demo/.state/inbox-seen.json");

    assert!(
        hp(
            home.path(),
            &["--root", root_arg, "context", "demo", "--peek"]
        )
        .status
        .success()
    );
    assert!(!seen.exists());
    assert!(
        hp(home.path(), &["--root", root_arg, "context", "demo"])
            .status
            .success()
    );
    assert!(
        std::fs::read_to_string(&seen)
            .unwrap()
            .contains("routine-r-1")
    );
}

#[test]
fn inbox_consume_outputs_and_archives_the_same_batch() {
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join("root");
    let root_arg = root.to_str().unwrap();
    assert!(
        hp(home.path(), &["--root", root_arg, "new", "demo"])
            .status
            .success()
    );
    let id = "20260917T000000Z-thread-state-t-0001-1";
    let item = format!(
        "+++\nid = \"{id}\"\nkind = \"thread-state\"\nsubject = \"t-0001\"\ncreated = \"x\"\nsummary = \"ready\"\n+++\n"
    );
    std::fs::write(root.join("demo/inbox").join(format!("{id}.md")), item).unwrap();

    let out = hp(
        home.path(),
        &["--root", root_arg, "inbox", "consume", "demo"],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(stdout.contains("[thread-state] t-0001: ready"));
    assert!(!root.join("demo/inbox").join(format!("{id}.md")).exists());
    assert!(
        root.join("demo/inbox/done")
            .join(format!("{id}.md"))
            .is_file()
    );
}

#[test]
fn path_like_names_and_slugs_are_refused() {
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join("root");
    let root_arg = root.to_str().unwrap();
    assert!(
        !hp(home.path(), &["--root", root_arg, "new", "../x"])
            .status
            .success()
    );
    assert!(
        !hp(home.path(), &["--root", root_arg, "open", "../x"])
            .status
            .success()
    );
    assert!(
        !hp(home.path(), &["--root", root_arg, "context", "../x"])
            .status
            .success()
    );
    assert!(
        !hp(home.path(), &["--root", root_arg, "thread", "list", "../x"])
            .status
            .success()
    );
    assert!(
        !hp(
            home.path(),
            &["--root", root_arg, "delete", "../x", "--force"]
        )
        .status
        .success()
    );
    assert!(!root.exists());
    assert!(!home.path().join("x").exists());
}

#[test]
fn ticker_start_without_projects_creates_nothing() {
    let home = tempfile::tempdir().unwrap();
    assert!(hp(home.path(), &["ticker", "start"]).status.success());
    assert!(!home.path().join(".herdr-projects").exists());
    assert!(!home.path().join(".config").exists());
}

#[test]
fn legacy_binary_alias_keeps_the_compatibility_data_root() {
    let home = tempfile::tempdir().unwrap();
    let output = Command::new(LEGACY_BIN)
        .env_clear()
        .env("HOME", home.path())
        .args(["new", "Legacy"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        home.path()
            .join(".herdr-projects/legacy/PROJECT.md")
            .is_file()
    );
}

#[test]
fn node_start_rejects_a_missing_parent_before_creating_state_or_starting_the_ticker() {
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join("root");
    let root_arg = root.to_str().unwrap();
    assert!(
        hp(home.path(), &["--root", root_arg, "new", "Demo"])
            .status
            .success()
    );
    let task = home.path().join("task.md");
    std::fs::write(&task, "A task that must not start").unwrap();
    let task_arg = task.to_str().unwrap();

    let output = hp(
        home.path(),
        &[
            "--root",
            root_arg,
            "node",
            "start",
            "demo",
            "--parent",
            "t-9999",
            "--role",
            "coordinator",
            "--title",
            "Rejected",
            "--task-file",
            task_arg,
        ],
    );
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("parent node `t-9999` does not exist")
    );
    assert!(!root.join("demo/threads/t-0001.toml").exists());
    assert!(!root.join("demo/nodes").exists());
    assert!(!root.join(".ticker.lock").exists());
}

#[test]
fn node_start_refuses_rules_from_standard_input_before_creating_anything() {
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join("root");
    let root_arg = root.to_str().unwrap();
    assert!(
        hp(home.path(), &["--root", root_arg, "new", "Demo"])
            .status
            .success()
    );
    let task = home.path().join("task.md");
    std::fs::write(&task, "A task").unwrap();
    let output = hp(
        home.path(),
        &[
            "--root",
            root_arg,
            "node",
            "start",
            "demo",
            "--title",
            "Child",
            "--task-file",
            task.to_str().unwrap(),
            "--rules-file",
            "-",
        ],
    );
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("--rules-file does not read standard input")
    );
    assert!(!root.join("demo/nodes").exists());
    assert!(!root.join(".ticker.lock").exists());
}

#[test]
fn node_rules_of_an_unknown_node_is_an_error() {
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join("root");
    let root_arg = root.to_str().unwrap();
    assert!(
        hp(home.path(), &["--root", root_arg, "new", "Demo"])
            .status
            .success()
    );
    let output = hp(
        home.path(),
        &["--root", root_arg, "node", "rules", "demo", "t-0001"],
    );
    assert!(!output.status.success());
}

#[test]
fn jump_waits_for_the_popup_to_exit_and_reports_a_failure_as_a_notification() {
    use std::os::unix::fs::PermissionsExt;
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join("root");
    let root_arg = root.to_str().unwrap();
    assert!(
        hp(home.path(), &["--root", root_arg, "new", "Demo"])
            .status
            .success()
    );
    // A herdr that records its arguments and refuses every call.
    let log = home.path().join("herdr.log");
    let herdr = home.path().join("herdr");
    std::fs::write(
        &herdr,
        format!("#!/bin/sh\necho \"$*\" >> '{}'\nexit 1\n", log.display()),
    )
    .unwrap();
    std::fs::set_permissions(&herdr, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut jump = Command::new(BIN)
        .env_clear()
        .env("HOME", home.path())
        .env("HERDR_BIN_PATH", &herdr)
        .env("HERDR_SOCKET_PATH", home.path().join("herdr.sock"))
        .args(["--root", root_arg, "jump", "node", "demo", "root"])
        .stdin(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert!(!log.exists(), "it called herdr while the popup was open");

    // The popup's process exits: its end of the pipe closes.
    drop(jump.stdin.take());
    assert!(jump.wait().unwrap().success());
    let calls = std::fs::read_to_string(&log).unwrap();
    assert!(
        calls
            .lines()
            .last()
            .unwrap()
            .starts_with("notification show Herdr Organizations --body "),
        "{calls}"
    );
}

/// Project `demo` with worker node `t-0001` on profile `ghost`, with rules
/// and memory in its scope.
fn node_fixture(home: &Path) -> std::path::PathBuf {
    let root = home.join("root");
    assert!(
        hp(home, &["--root", root.to_str().unwrap(), "new", "Demo"])
            .status
            .success()
    );
    let demo = root.join("demo");
    std::fs::write(
        demo.join("threads/t-0001.toml"),
        "id = \"t-0001\"\ntitle = \"Review\"\nstatus = \"open\"\nkind = \"tab\"\nagent = \"claude\"\nprofile = \"ghost\"\n",
    )
    .unwrap();
    let scope = demo.join("nodes/t-0001");
    std::fs::create_dir_all(scope.join("memory")).unwrap();
    std::fs::write(
        scope.join("INSTRUCTIONS.md"),
        "# Node instructions\n\nCite the line you mean.\n",
    )
    .unwrap();
    std::fs::write(scope.join("MEMORY.md"), "# Node memory\n\n- CI is slow.\n").unwrap();
    std::fs::write(scope.join("memory/ci.md"), "Retry once.\n").unwrap();
    root
}

#[test]
fn template_commands_save_list_show_and_delete_a_node_template() {
    let home = tempfile::tempdir().unwrap();
    let root = node_fixture(home.path());
    let run = |args: &[&str]| {
        let mut all = vec!["--root", root.to_str().unwrap()];
        all.extend_from_slice(args);
        hp(home.path(), &all)
    };
    let text = |output: std::process::Output| {
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    };

    text(run(&[
        "template", "save", "demo", "t-0001", "--name", "reviewer",
    ]));
    let shown = text(run(&["template", "show", "reviewer", "--project", "demo"]));
    assert!(
        shown.starts_with("reviewer (project demo)\nrole: worker\nprofile: ghost\n"),
        "{shown}"
    );
    assert!(
        shown.contains("## Rules\n\nCite the line you mean.\n"),
        "{shown}"
    );
    assert!(shown.contains("- CI is slow."), "{shown}");
    assert!(
        shown.contains("### memory/ci.md\n\nRetry once.\n"),
        "{shown}"
    );

    let again = run(&["template", "save", "demo", "t-0001", "--name", "reviewer"]);
    assert!(!again.status.success());
    assert!(String::from_utf8_lossy(&again.stderr).contains("--force replaces it"));
    text(run(&[
        "template", "save", "demo", "t-0001", "--name", "reviewer", "--force",
    ]));
    text(run(&[
        "template",
        "save",
        "demo",
        "t-0001",
        "--name",
        "reviewer",
        "--global",
        "--description",
        "Careful reviews",
    ]));
    assert!(
        home.path()
            .join(".config/herdr-projects/templates/reviewer/TEMPLATE.toml")
            .is_file()
    );

    assert_eq!(
        text(run(&["template", "list", "--project", "demo"])),
        "reviewer\tproject\tworker\tghost\t\n"
    );
    text(run(&[
        "template",
        "delete",
        "reviewer",
        "--project",
        "demo",
    ]));
    assert_eq!(
        text(run(&["template", "list", "--project", "demo"])),
        "reviewer\tglobal\tworker\tghost\tCareful reviews\n"
    );
    let gone = run(&["template", "delete", "reviewer", "--project", "demo"]);
    assert!(!gone.status.success());
    text(run(&["template", "delete", "reviewer"]));
    assert_eq!(text(run(&["template", "list"])), "");
}

#[test]
fn node_start_applies_a_template_in_the_form_nenu_sends() {
    let home = tempfile::tempdir().unwrap();
    let root = node_fixture(home.path());
    let root_arg = root.to_str().unwrap();
    assert!(
        hp(
            home.path(),
            &[
                "--root", root_arg, "template", "save", "demo", "t-0001", "--name", "reviewer"
            ],
        )
        .status
        .success()
    );
    let start = |template: &str| {
        let task = home.path().join("task.md");
        std::fs::write(&task, "A task").unwrap();
        hp(
            home.path(),
            &[
                "--root",
                root_arg,
                "node",
                "start",
                "demo",
                template,
                "--parent=root",
                "--title=Child",
                "--task-file",
                task.to_str().unwrap(),
            ],
        )
    };

    // The template's profile is applied, and refused before anything starts.
    let output = start("--template=reviewer");
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("there is no profile `ghost`"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let output = start("--template=nope");
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("there is no template `nope`; `template list --project demo` shows them")
    );
    assert!(!root.join("demo/threads/t-0002.toml").exists());
    assert!(!root.join(".ticker.lock").exists());
}

#[cfg(unix)]
mod progress_hooks {
    use super::*;
    use serde_json::{Value, json};
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;
    use std::process::Stdio;

    /// What the hook does in the pane under test.
    #[derive(Clone, Copy, PartialEq)]
    enum Speaks {
        Never,
        Worker,
        Coordinator,
    }

    struct Hooks {
        home: tempfile::TempDir,
        root: std::path::PathBuf,
        herdr: std::path::PathBuf,
        pane: Value,
        coordinator: Value,
        codex_thread_id: Option<&'static str>,
        agent_reply: Option<Value>,
    }

    impl Hooks {
        fn new() -> Self {
            let home = tempfile::tempdir().unwrap();
            let root = home.path().join("root");
            assert!(
                hp(
                    home.path(),
                    &["--root", root.to_str().unwrap(), "new", "demo"]
                )
                .status
                .success()
            );
            let cwd = root.join("demo").canonicalize().unwrap();
            let pane = json!({"pane_id":"w1:p1", "terminal_id":"term-1", "workspace_id":"w1", "tab_id":"w1:t1", "cwd":cwd, "agent":"codex", "name":"hpc-demo"});
            let coordinator = json!({"socket":"/managed.sock", "pane_id":"w1:p1", "workspace_id":"w1", "tab_id":"w1:t1", "cwd":cwd, "agent":"codex"});
            let herdr = home.path().join("herdr");
            std::fs::write(&herdr, "#!/bin/sh\necho \"$*\" >> \"$FIXTURES/calls.log\"\ncase \"$*\" in\n  'pane current --current') /bin/cat \"$FIXTURES/current.json\" ;;\n  'pane list') /bin/cat \"$FIXTURES/panes.json\" ;;\n  'agent list') /bin/cat \"$FIXTURES/agents.json\" ;;\n  *) exit 1 ;;\nesac\n").unwrap();
            std::fs::set_permissions(&herdr, std::fs::Permissions::from_mode(0o755)).unwrap();
            let fixture = Self {
                home,
                root,
                herdr,
                pane,
                coordinator,
                codex_thread_id: Some("main"),
                agent_reply: None,
            };
            fixture.save_coordinator();
            fixture
        }

        fn save_coordinator(&self) {
            std::fs::write(
                self.root.join("demo/.state/coordinator.json"),
                self.coordinator.to_string(),
            )
            .unwrap();
        }

        fn thread(&self, machine: &str) {
            self.node(machine, "worker");
        }

        /// A node record in the pane: `worker` is a plain thread, `coordinator`
        /// a coordinator node with children allowed.
        fn node(&self, machine: &str, role: &str) {
            let can_spawn = role == "coordinator";
            let text = format!(
                "id = 't-0001'\nparent_id = 'root'\nrole = '{role}'\ncan_spawn = {can_spawn}\nstatus = 'open'\nkind = 'worktree'\npane_id = 'w1:p1'\nworkspace_id = 'w1'\ntab_id = 'w1:t1'\ncwd = '/separate/worktree'\nagent = 'codex'\nagent_name = 'hpt-demo-t-0001'\nmachine = '{machine}'\n"
            );
            std::fs::write(self.root.join("demo/threads/t-0001.toml"), text).unwrap();
        }

        fn run(&self, socket: &str, event: Value) -> std::process::Output {
            for (file, reply) in [
                ("current.json", json!({"result":{"pane":self.pane}})),
                ("panes.json", json!({"result":{"panes":[self.pane]}})),
                (
                    "agents.json",
                    self.agent_reply
                        .clone()
                        .unwrap_or_else(|| json!({"result":{"agents":[self.pane]}})),
                ),
            ] {
                std::fs::write(self.home.path().join(file), reply.to_string()).unwrap();
            }
            let mut command = Command::new(BIN);
            command
                .env_clear()
                .env("HOME", self.home.path())
                .env("HERDR_ENV", "1")
                .env("HERDR_PANE_ID", "w1:p1")
                .env("HERDR_SOCKET_PATH", socket)
                .env("HERDR_BIN_PATH", &self.herdr)
                .env("FIXTURES", self.home.path())
                .args([
                    "--root",
                    self.root.to_str().unwrap(),
                    "hook",
                    "--agent",
                    "codex",
                ])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            if let Some(id) = self.codex_thread_id {
                command.env("CODEX_THREAD_ID", id);
            }
            let mut child = command.spawn().unwrap();
            child
                .stdin
                .take()
                .unwrap()
                .write_all(event.to_string().as_bytes())
                .unwrap();
            let out = child.wait_with_output().unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            assert!(out.stderr.is_empty());
            out
        }

        fn check_events(&self, socket: &str, pane: Speaks) {
            for kind in ["SessionStart", "UserPromptSubmit", "PostToolUse"] {
                let progress = self.root.join(".progress");
                if progress.exists() {
                    std::fs::remove_dir_all(&progress).unwrap();
                }
                let out = self.run(socket, json!({"hook_event_name":kind, "session_id":"main"}));
                // A coordinator pane gets no check-in reminder after a tool
                // call, so nothing in its record changes either.
                let speaks = match pane {
                    Speaks::Never => false,
                    Speaks::Worker => true,
                    Speaks::Coordinator => kind != "PostToolUse",
                };
                assert_eq!(
                    !out.stdout.is_empty(),
                    speaks,
                    "{kind}: {}",
                    String::from_utf8_lossy(&out.stdout)
                );
                assert_eq!(
                    progress.exists(),
                    speaks,
                    "{kind}: progress record creation"
                );
                if speaks {
                    let response: Value = serde_json::from_slice(&out.stdout).unwrap();
                    assert!(
                        !response["hookSpecificOutput"]["additionalContext"]
                            .as_str()
                            .unwrap()
                            .is_empty()
                    );
                }
            }
        }
    }

    #[test]
    fn unmanaged_progress_hooks_are_silent_for_every_event() {
        let mut h = Hooks::new();
        h.pane["cwd"] = "/unrelated/repo".into();
        h.check_events("/managed.sock", Speaks::Never);
        h.pane["cwd"] = h.coordinator["cwd"].clone();
        h.check_events("/other.sock", Speaks::Never);
    }

    #[test]
    fn an_unmanaged_pane_costs_no_herdr_call_beyond_its_own() {
        let mut h = Hooks::new();
        h.pane["cwd"] = "/unrelated/repo".into();
        let log = h.home.path().join("calls.log");
        for kind in ["SessionStart", "UserPromptSubmit", "PostToolUse"] {
            let _ = std::fs::remove_file(&log);
            assert!(
                h.run(
                    "/managed.sock",
                    json!({"hook_event_name":kind, "session_id":"main"})
                )
                .stdout
                .is_empty()
            );
            let calls = std::fs::read_to_string(&log).unwrap_or_default();
            assert!(
                calls.lines().all(|c| c == "pane current --current"),
                "{kind}: {calls}"
            );
        }
    }

    #[test]
    fn managed_progress_hooks_cover_coordinators_discovery_and_worktrees() {
        let mut h = Hooks::new();
        h.check_events("/managed.sock", Speaks::Coordinator);
        // A second coordinator before the ticker has recorded it.
        h.coordinator["pane_id"] = "w9:p1".into();
        h.save_coordinator();
        h.pane["foreground_cwd"] = h.pane["cwd"].clone();
        h.pane["cwd"] = "/shell".into();
        h.check_events("/managed.sock", Speaks::Coordinator);
        std::fs::remove_file(h.root.join("demo/.state/coordinator.json")).unwrap();
        h.check_events("/managed.sock", Speaks::Coordinator);
        h.save_coordinator();
        h.pane["foreground_cwd"] = "".into();
        h.pane["cwd"] = "/separate/worktree".into();
        h.pane["name"] = "hpt-demo-t-0001".into();
        h.thread("");
        h.check_events("/managed.sock", Speaks::Worker);
        h.check_events("/other.sock", Speaks::Never);
        // A remote pane id must never grant ownership to a local pane.
        h.thread("remote-host");
        h.check_events("/managed.sock", Speaks::Never);
    }

    #[test]
    fn a_prompt_the_ticker_typed_gets_no_reminder_and_one_the_user_typed_does() {
        let h = Hooks::new();
        let prompt = |text: &str| {
            h.run(
                "/managed.sock",
                json!({"hook_event_name":"UserPromptSubmit", "session_id":"main", "prompt":text}),
            )
            .stdout
        };
        assert!(prompt("[hp inbox] t-0001 new report").is_empty());
        assert!(prompt("<task-notification>\n<status>completed</status>").is_empty());
        assert!(!prompt("Start the release").is_empty());
    }

    #[test]
    fn coordinator_node_panes_speak_and_stay_silent_like_threads() {
        let mut h = Hooks::new();
        // The root coordinator is in another pane: only the node record ties this one.
        h.coordinator["pane_id"] = "w9:p1".into();
        h.save_coordinator();
        h.pane["cwd"] = "/separate/worktree".into();
        h.pane["name"] = "hpt-demo-t-0001".into();
        h.node("", "coordinator");
        h.check_events("/managed.sock", Speaks::Coordinator);
        h.check_events("/other.sock", Speaks::Never);
        // Its pane id reused by a pane in another folder is not the node's.
        h.pane["cwd"] = "/different/worktree".into();
        h.check_events("/managed.sock", Speaks::Never);
        // A resolved coordinator node no longer speaks.
        h.pane["cwd"] = "/separate/worktree".into();
        let file = h.root.join("demo/threads/t-0001.toml");
        let text = std::fs::read_to_string(&file).unwrap();
        std::fs::write(file, text.replace("status = 'open'", "status = 'resolved'")).unwrap();
        h.check_events("/managed.sock", Speaks::Never);
    }

    #[test]
    fn managed_progress_hooks_keep_subagent_and_foreign_session_exclusions() {
        let mut h = Hooks::new();
        for event in [
            json!({"hook_event_name":"SessionStart", "session_id":"nested"}),
            json!({"hook_event_name":"SessionStart", "session_id":"main", "agent_id":"child"}),
        ] {
            assert!(h.run("/managed.sock", event).stdout.is_empty());
            assert!(!h.root.join(".progress").exists());
        }
        assert!(
            !h.run(
                "/managed.sock",
                json!({"hook_event_name":"SessionStart", "session_id":"main"})
            )
            .stdout
            .is_empty()
        );
        let file = std::fs::read_dir(h.root.join(".progress"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let before = std::fs::read(&file).unwrap();
        // Exercise the saved session exclusion independently of Codex's env gate.
        h.codex_thread_id = None;
        assert!(
            h.run(
                "/managed.sock",
                json!({"hook_event_name":"PostToolUse", "session_id":"nested"})
            )
            .stdout
            .is_empty()
        );
        assert_eq!(std::fs::read(file).unwrap(), before);
    }

    #[test]
    fn unmanaged_progress_hooks_leave_existing_records_unchanged() {
        let mut h = Hooks::new();
        h.run(
            "/managed.sock",
            json!({"hook_event_name":"SessionStart", "session_id":"main"}),
        );
        let file = std::fs::read_dir(h.root.join(".progress"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let before = std::fs::read(&file).unwrap();
        h.pane["cwd"] = "/unrelated/repo".into();
        for kind in ["SessionStart", "UserPromptSubmit", "PostToolUse"] {
            assert!(
                h.run(
                    "/managed.sock",
                    json!({"hook_event_name":kind, "session_id":"main"})
                )
                .stdout
                .is_empty()
            );
            assert_eq!(std::fs::read(&file).unwrap(), before);
        }
    }

    #[test]
    fn managed_progress_hooks_reject_reused_thread_identity() {
        let mut h = Hooks::new();
        h.coordinator["pane_id"] = "w9:p1".into();
        h.save_coordinator();
        h.pane["cwd"] = "/separate/worktree".into();
        h.pane["name"] = "hpt-demo-t-0001".into();
        h.thread("");
        h.pane["name"] = "someone-else".into();
        h.check_events("/managed.sock", Speaks::Never);
        h.pane["name"] = "hpt-demo-t-0001".into();
        h.pane["cwd"] = "/different/worktree".into();
        h.check_events("/managed.sock", Speaks::Never);
        h.pane["cwd"] = "/separate/worktree".into();
        let file = h.root.join("demo/threads/t-0001.toml");
        let text = std::fs::read_to_string(&file).unwrap();
        std::fs::write(file, text.replace("status = 'open'", "status = 'resolved'")).unwrap();
        h.check_events("/managed.sock", Speaks::Never);
    }

    #[test]
    fn managed_progress_hooks_discover_coordinators_before_agent_detection() {
        let mut h = Hooks::new();
        std::fs::remove_file(h.root.join("demo/.state/coordinator.json")).unwrap();
        h.pane["agent"] = "".into();
        h.agent_reply = Some(json!({"result":{"agents":[]}}));
        let out = h.run(
            "/managed.sock",
            json!({"hook_event_name":"SessionStart", "session_id":"main"}),
        );
        assert!(String::from_utf8_lossy(&out.stdout).contains("# Progress (herdr-projects)"));
        assert!(
            !h.root.join("demo/.state/coordinator.json").exists(),
            "discovery stays read-only"
        );
    }

    #[test]
    fn managed_progress_hooks_reject_failed_agent_observations() {
        let mut h = Hooks::new();
        h.coordinator["pane_id"] = "w9:p1".into();
        h.save_coordinator();
        h.pane["cwd"] = "/separate/worktree".into();
        h.pane["agent"] = "".into();
        h.thread("");
        for reply in [
            json!({"error":{"code":"timeout", "message":"unavailable"}}),
            json!({"result":{"agents":null}}),
        ] {
            h.agent_reply = Some(reply);
            assert!(
                h.run(
                    "/managed.sock",
                    json!({"hook_event_name":"PostToolUse", "session_id":"main"})
                )
                .stdout
                .is_empty()
            );
            assert!(!h.root.join(".progress").exists());
        }
    }
}

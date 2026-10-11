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

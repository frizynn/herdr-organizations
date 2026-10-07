//! End-to-end checks of the built binary with a scrubbed environment.

use std::path::Path;
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_herdr-projects");

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
fn thread_summary_prints_the_report_header() {
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join("root");
    let root_arg = root.to_str().unwrap();
    assert!(
        hp(home.path(), &["--root", root_arg, "new", "demo"])
            .status
            .success()
    );
    let threads = root.join("demo/threads");
    std::fs::create_dir_all(&threads).unwrap();
    std::fs::write(threads.join("t-0001.toml"), "id = \"t-0001\"\n").unwrap();

    let missing_report = hp(
        home.path(),
        &["--root", root_arg, "node", "summary", "demo", "t-0001"],
    );
    assert!(missing_report.status.success());
    assert_eq!(
        String::from_utf8(missing_report.stdout).unwrap(),
        "no report yet\n"
    );

    let report =
        "PR: none\nStatus: in-progress, not verified\nNeeds: nothing\n\n## Report\nDetails.\n";
    std::fs::write(threads.join("t-0001.md"), report).unwrap();
    let output = hp(
        home.path(),
        &["--root", root_arg, "thread", "summary", "demo", "t-0001"],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "PR: none | Status: in-progress, not verified | Needs: nothing\n"
    );
}

#[test]
fn node_rules_reads_and_replaces_instructions() {
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join("root");
    let root_arg = root.to_str().unwrap();
    assert!(
        hp(home.path(), &["--root", root_arg, "new", "demo"])
            .status
            .success()
    );
    let threads = root.join("demo/threads");
    std::fs::create_dir_all(&threads).unwrap();
    std::fs::write(threads.join("t-0001.toml"), "id = \"t-0001\"\n").unwrap();
    let instructions = root.join("demo/nodes/t-0001/INSTRUCTIONS.md");
    std::fs::create_dir_all(instructions.parent().unwrap()).unwrap();
    std::fs::write(&instructions, "# Node instructions\n\nOriginal rules.\n").unwrap();

    let read = hp(
        home.path(),
        &["--root", root_arg, "node", "rules", "demo", "t-0001"],
    );
    assert!(read.status.success());
    assert_eq!(
        String::from_utf8(read.stdout).unwrap(),
        "# Node instructions\n\nOriginal rules.\n"
    );

    let rules_file = home.path().join("rules.md");
    std::fs::write(&rules_file, "  Keep reports short.  \n").unwrap();
    let replace = hp(
        home.path(),
        &[
            "--root",
            root_arg,
            "node",
            "rules",
            "demo",
            "t-0001",
            "--text-file",
            rules_file.to_str().unwrap(),
        ],
    );
    assert!(
        replace.status.success(),
        "{}",
        String::from_utf8_lossy(&replace.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(instructions).unwrap(),
        "# Node instructions\n\nKeep reports short.\n"
    );
}

#[test]
fn node_start_rejects_rules_file_stdin_before_starting_the_ticker() {
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join("root");
    let root_arg = root.to_str().unwrap();
    assert!(
        hp(home.path(), &["--root", root_arg, "new", "demo"])
            .status
            .success()
    );
    let task_file = home.path().join("task.md");
    std::fs::write(&task_file, "Task body.\n").unwrap();

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
            task_file.to_str().unwrap(),
            "--rules-file",
            "-",
        ],
    );
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("--rules-file does not read standard input; --task-file may")
    );
    assert!(!root.join(".ticker.lock").exists());
    assert!(!root.join("demo/threads/t-0001.toml").exists());
}

#[test]
fn node_start_rejects_template_with_rules_file_before_starting_the_ticker() {
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join("root");
    let root_arg = root.to_str().unwrap();
    assert!(
        hp(home.path(), &["--root", root_arg, "new", "demo"])
            .status
            .success()
    );
    save_template_cli(home.path(), &root, "x", "Template rules.");
    let task_file = home.path().join("task.md");
    let rules_file = home.path().join("rules.md");
    std::fs::write(&task_file, "Task body.\n").unwrap();
    std::fs::write(&rules_file, "Other rules.\n").unwrap();

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
            task_file.to_str().unwrap(),
            "--template",
            "x",
            "--rules-file",
            rules_file.to_str().unwrap(),
        ],
    );

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("pass --rules-file or --template, not both")
    );
    assert!(!root.join(".ticker.lock").exists());
    assert!(!root.join("demo/threads/t-0001.toml").exists());
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
fn the_binary_keeps_the_herdr_projects_data_root() {
    let home = tempfile::tempdir().unwrap();
    let output = Command::new(BIN)
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

fn save_template_cli(home: &Path, root: &Path, name: &str, rules: &str) {
    let rules_file = home.join(format!("{name}-rules.md"));
    std::fs::write(&rules_file, rules).unwrap();
    let output = hp(
        home,
        &[
            "--root",
            root.to_str().unwrap(),
            "template",
            "save",
            name,
            "--role",
            "coordinator",
            "--rules-file",
            rules_file.to_str().unwrap(),
        ],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn template_save_and_list_json_have_the_exact_contract_keys() {
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join("root");
    save_template_cli(home.path(), &root, "x", "Keep the plan short.");

    let output = hp(
        home.path(),
        &[
            "--root",
            root.to_str().unwrap(),
            "template",
            "list",
            "--json",
        ],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let rows: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let row = rows
        .as_array()
        .unwrap()
        .first()
        .unwrap()
        .as_object()
        .unwrap();
    let mut keys: Vec<_> = row.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        vec![
            "can_spawn",
            "description",
            "dir",
            "memory_chars",
            "name",
            "profile",
            "project",
            "role",
            "rules_chars",
            "scope",
            "updated",
        ]
    );
}

#[test]
fn template_show_json_includes_rules() {
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join("root");
    save_template_cli(home.path(), &root, "x", "Keep the plan short.");

    let output = hp(
        home.path(),
        &[
            "--root",
            root.to_str().unwrap(),
            "template",
            "show",
            "x",
            "--json",
        ],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let shown: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(shown["rules"], "Keep the plan short.");
}

#[test]
fn template_memory_can_be_replaced_and_read() {
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join("root");
    save_template_cli(home.path(), &root, "x", "");
    let memory_file = home.path().join("memory.md");
    std::fs::write(&memory_file, "Progress: API is ready.\n").unwrap();

    let replaced = hp(
        home.path(),
        &[
            "--root",
            root.to_str().unwrap(),
            "template",
            "memory",
            "x",
            "--text-file",
            memory_file.to_str().unwrap(),
        ],
    );
    assert!(
        replaced.status.success(),
        "{}",
        String::from_utf8_lossy(&replaced.stderr)
    );

    let output = hp(
        home.path(),
        &["--root", root.to_str().unwrap(), "template", "memory", "x"],
    );
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "Progress: API is ready.\n"
    );
}

#[test]
fn template_delete_removes_the_template_and_show_fails() {
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join("root");
    save_template_cli(home.path(), &root, "x", "");

    let deleted = hp(
        home.path(),
        &["--root", root.to_str().unwrap(), "template", "delete", "x"],
    );
    assert!(
        deleted.status.success(),
        "{}",
        String::from_utf8_lossy(&deleted.stderr)
    );

    let shown = hp(
        home.path(),
        &["--root", root.to_str().unwrap(), "template", "show", "x"],
    );
    assert!(!shown.status.success());
    assert!(String::from_utf8_lossy(&shown.stderr).contains("template `x` not found"));
}

#[test]
fn gate_run_queues_on_one_slot_and_passes_the_exit_code_through() {
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join("root");
    let root_arg = root.to_str().unwrap();
    let marker = home.path().join("first-done");
    let first = std::process::Command::new(BIN)
        .env("HOME", home.path())
        .args(["--root", root_arg, "gate", "run", "--", "sh", "-c"])
        .arg(format!("sleep 1; touch {}", marker.display()))
        .spawn()
        .unwrap();
    // Wait until the first command holds the slot.
    let lock = root.join(".gates/heavy-0.lock");
    for _ in 0..100 {
        if std::fs::read_to_string(&lock).is_ok_and(|t| t.contains("pid")) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let second = hp(
        home.path(),
        &[
            "--root",
            root_arg,
            "gate",
            "run",
            "--",
            "sh",
            "-c",
            &format!("test -e {} && exit 7", marker.display()),
        ],
    );
    assert_eq!(
        second.status.code(),
        Some(7),
        "the second ran before the first finished"
    );
    assert!(String::from_utf8_lossy(&second.stderr).contains("gate heavy: waiting"));
    let mut first = first;
    assert!(first.wait().unwrap().success());
    let none = hp(home.path(), &["--root", root_arg, "gate", "run"]);
    assert!(!none.status.success());
}

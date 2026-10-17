//! Tasks per coordinator: each coordinator owns a list (the root `TASKS.md`,
//! a coordinator node `nodes/<id>/TASKS.md`, or a tracker's list for the
//! node's task scope), each task has the nodes assigned to it, and `task
//! list` shows the project's view or one coordinator's. Checked against the
//! built binary with a scrubbed environment; the tracker is a shell script.

use std::path::PathBuf;
use std::process::{Command, Output};

use serde_json::{Value, json};

const BIN: &str = env!("CARGO_BIN_EXE_herdr-organizations");

struct Fixture {
    home: tempfile::TempDir,
    root: PathBuf,
}

impl Fixture {
    fn new() -> Fixture {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join("root");
        std::fs::create_dir_all(&root).unwrap();
        let root = std::fs::canonicalize(root).unwrap();
        let fixture = Fixture { home, root };
        std::fs::create_dir_all(fixture.home.path().join(".config/herdr-projects")).unwrap();
        fixture
    }

    fn path(&self, name: &str) -> PathBuf {
        self.home.path().join(name)
    }

    fn dir(&self) -> PathBuf {
        self.root.join("acme")
    }

    fn write(&self, file: &str, text: &str) {
        let path = self.dir().join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn script(&self, name: &str, body: &str) {
        use std::os::unix::fs::PermissionsExt;
        let path = self.path(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn config(&self, text: &str) {
        std::fs::write(
            self.home.path().join(".config/herdr-projects/config.toml"),
            text,
        )
        .unwrap();
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(BIN)
            .env_clear()
            .env("HOME", self.home.path())
            .arg("--root")
            .arg(&self.root)
            .args(args)
            .output()
            .unwrap()
    }

    fn text(&self, args: &[&str]) -> String {
        let out = self.run(args);
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    }

    fn json(&self, args: &[&str]) -> Value {
        serde_json::from_str(&self.text(args)).unwrap()
    }

    /// A node record under `parent`; `extra` is more TOML.
    fn node(&self, id: &str, title: &str, parent: &str, role: &str, extra: &str) {
        let spawn = if role == "coordinator" {
            "can_spawn = true\n"
        } else {
            ""
        };
        self.write(
            &format!("threads/{id}.toml"),
            &format!(
                "id = \"{id}\"\ntitle = \"{title}\"\nparent_id = \"{parent}\"\nrole = \"{role}\"\n{spawn}status = \"open\"\nkind = \"tab\"\nagent = \"claude\"\ncreated = \"2026-10-01T10:00:00Z\"\nlast_group = \"working\"\n{extra}"
            ),
        );
        self.write(&format!("threads/{id}.task.md"), "task\n");
    }

    fn calls(&self) -> Vec<String> {
        std::fs::read_to_string(self.path("calls.log"))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }
}

fn titles(tasks: &Value) -> Vec<String> {
    tasks
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["title"].as_str().unwrap().to_string())
        .collect()
}

/// Without a tracker the root keeps `TASKS.md` and each coordinator node
/// its own `nodes/<id>/TASKS.md`; a node's assignments put it on a task, a
/// coordinator child on a task makes it delegated in the parent and its own
/// in the child, and a list whose coordinator is resolved has no owner.
fn two_coordinators() -> Fixture {
    let fixture = Fixture::new();
    assert!(fixture.run(&["new", "acme"]).status.success());
    fixture.write(
        "TASKS.md",
        "# Tasks\n\n## Now\n- [ ] Ship billing\n- [ ] Plan the quarter (me)\n",
    );
    fixture.node(
        "t-0001",
        "Billing",
        "root",
        "coordinator",
        "tasks = [\"Ship billing\"]\n",
    );
    fixture.node("t-0002", "Docs", "root", "coordinator", "");
    fixture.node(
        "t-0003",
        "Charge cards",
        "t-0001",
        "worker",
        "tasks = [\"Charge cards\", \"ship billing\"]\n",
    );
    fixture.node("t-0004", "Refund helper", "t-0001", "worker", "");
    fixture.node("t-0005", "Old area", "root", "coordinator", "");
    fixture.write(
        "threads/t-0005.toml",
        &std::fs::read_to_string(fixture.dir().join("threads/t-0005.toml"))
            .unwrap()
            .replace("status = \"open\"", "status = \"resolved\""),
    );
    fixture.write(
        "nodes/t-0001/TASKS.md",
        "## Now\n- [ ] Charge cards\n- [ ] Refunds · t-0004\n",
    );
    fixture.write("nodes/t-0002/TASKS.md", "- [ ] Write the guide\n");
    fixture.write("nodes/t-0005/TASKS.md", "- [ ] Old thing\n");
    fixture
}

#[test]
fn the_project_view_groups_the_tasks_by_coordinator_and_keeps_the_unowned() {
    let fixture = two_coordinators();
    let listed = fixture.json(&["task", "list", "acme", "--json"]);
    // The root's tasks stay where they were.
    assert_eq!(
        titles(&listed["tasks"]),
        ["Ship billing", "Plan the quarter"]
    );
    let owners: Vec<(String, String, Vec<String>)> = listed["owners"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| {
            (
                o["id"].as_str().unwrap().to_string(),
                o["title"].as_str().unwrap().to_string(),
                titles(&o["tasks"]),
            )
        })
        .collect();
    let s = |v: &str| v.to_string();
    assert_eq!(
        owners,
        [
            (
                s("root"),
                s(""),
                vec![s("Ship billing"), s("Plan the quarter")]
            ),
            (
                s("t-0001"),
                s("Billing"),
                vec![s("Charge cards"), s("Refunds")]
            ),
            (s("t-0002"), s("Docs"), vec![s("Write the guide")]),
        ]
    );
    assert_eq!(titles(&listed["unowned"]), ["Old thing"]);
    assert_eq!(listed["unowned"][0]["owner_node"], "t-0005");

    // Ship billing: delegated to t-0001, with t-0003 under it.
    let ship = &listed["owners"][0]["tasks"][0];
    assert_eq!(ship["delegated"], true);
    assert_eq!(ship["assignees"].as_array().unwrap().len(), 1);
    assert_eq!(ship["assignees"][0]["id"], "t-0001");
    assert_eq!(ship["assignees"][0]["role"], "coordinator");
    assert_eq!(ship["assignees"][0]["assignees"][0]["id"], "t-0003");
    assert_eq!(listed["owners"][0]["tasks"][1]["assignees"], json!([]));
    // The task line's thread is an assignee too.
    let refunds = &listed["owners"][1]["tasks"][1];
    assert_eq!(refunds["assignees"][0]["id"], "t-0004");
    assert_eq!(refunds["delegated"], false);

    let text = fixture.text(&["task", "list", "acme"]);
    assert!(text.starts_with("## root\n"), "{text}");
    assert!(text.contains("## t-0001 Billing\n"), "{text}");
    assert!(text.contains("## unowned\n"), "{text}");
    assert!(
        text.contains("Now\tShip billing\t\t\tdelegated: t-0001 (t-0003)\n"),
        "{text}"
    );
}

#[test]
fn a_coordinator_view_lists_its_tasks_and_those_delegated_to_it() {
    let fixture = two_coordinators();
    let view = fixture.json(&["task", "list", "acme", "--node", "t-0001", "--json"]);
    assert_eq!(view["node"], json!({"id": "t-0001", "title": "Billing"}));
    assert_eq!(
        titles(&view["tasks"]),
        ["Charge cards", "Refunds", "Ship billing"]
    );
    let via: Vec<&str> = view["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["via"].as_str().unwrap())
        .collect();
    assert_eq!(via, ["own", "own", "assigned"]);
    // In the child's own view, the delegated task's people are its own.
    assert_eq!(view["tasks"][2]["assignees"][0]["id"], "t-0003");
    assert_eq!(view["tasks"][2]["owner_node"], "root");
    assert_eq!(view["tasks"][0]["assignees"][0]["id"], "t-0003");
    let worker = &view["tasks"][0]["assignees"][0];
    assert_eq!(worker["group"], "working");
    assert!(worker.get("verdict").is_some());

    let root = fixture.json(&["task", "list", "acme", "--node", "root", "--json"]);
    assert_eq!(titles(&root["tasks"]), ["Ship billing", "Plan the quarter"]);
    assert!(
        !fixture
            .run(&["task", "list", "acme", "--node", "t-0003"])
            .status
            .success()
    );
}

/// `node assign` and `node unassign` change only the record, refuse a task
/// that does not exist, and a node may be on several tasks.
#[test]
fn node_assign_puts_a_node_on_several_tasks_and_unassign_takes_it_off() {
    let fixture = two_coordinators();
    let assign = |task: &str| fixture.run(&["node", "assign", "acme", "t-0002", "--task", task]);
    assert!(assign("Write the guide").status.success());
    assert!(assign("Plan the quarter").status.success());
    assert!(
        assign("Plan the quarter").status.success(),
        "a repeat is a no-op"
    );
    let missing = assign("No such task");
    assert!(!missing.status.success());
    assert!(String::from_utf8_lossy(&missing.stderr).contains("No such task"));
    let record = std::fs::read_to_string(fixture.dir().join("threads/t-0002.toml")).unwrap();
    assert!(
        record.contains("tasks = [\"Write the guide\", \"Plan the quarter\"]"),
        "{record}"
    );
    let shown = fixture.json(&["node", "show", "acme", "t-0002", "--json"]);
    assert_eq!(
        shown["thread"]["tasks"],
        json!(["Write the guide", "Plan the quarter"])
    );
    assert!(
        fixture
            .run(&[
                "node",
                "unassign",
                "acme",
                "t-0002",
                "--task",
                "plan the quarter"
            ])
            .status
            .success()
    );
    let shown = fixture.json(&["node", "show", "acme", "t-0002", "--json"]);
    assert_eq!(shown["thread"]["tasks"], json!(["Write the guide"]));
}

/// A project with nothing new prints its tasks as before: no grouping, and
/// the document has only `tasks`.
#[test]
fn a_project_without_coordinator_nodes_lists_its_tasks_as_before() {
    let fixture = Fixture::new();
    assert!(fixture.run(&["new", "acme"]).status.success());
    fixture.write("TASKS.md", "# Tasks\n\n## Now\n- [ ] One (me)\n");
    fixture.node("t-0001", "Worker", "root", "worker", "");
    assert_eq!(fixture.text(&["task", "list", "acme"]), "Now\tOne\tme\t\n");
    let listed = fixture.json(&["task", "list", "acme", "--json"]);
    let keys: Vec<&String> = listed.as_object().unwrap().keys().collect();
    assert_eq!(keys, ["project", "schema_version", "tasks"]);
    let shown = fixture.json(&["node", "show", "acme", "t-0001", "--json"]);
    assert!(shown["thread"].get("tasks").is_none(), "{shown}");
    assert!(shown["thread"].get("task_scope").is_none(), "{shown}");
}

const TRACKED: &str = r#"[
  {"id":"B-1","title":"Charge cards","list":"Billing"},
  {"id":"B-2","title":"Refunds","list":"Billing"},
  {"id":"G-1","title":"Hire","list":"General"}
]"#;

/// A tracker: `list` prints the billing tasks for the scope `billing` and
/// every task for an empty scope, and `get` (when `with_get`) knows one more
/// task that no list prints. Every call is logged with its node and scope.
fn tracked(with_get: bool) -> Fixture {
    let fixture = Fixture::new();
    std::fs::write(fixture.path("all.json"), TRACKED).unwrap();
    fixture.script(
        "list.sh",
        &format!(
            "printf 'list %s [%s]\\n' \"$HP_NODE_ID\" \"$HP_TASK_SCOPE\" >> {log}\nif [ \"$HP_TASK_SCOPE\" = billing ]; then\n  printf '{{\"schema_version\":1,\"tasks\":[{{\"id\":\"B-1\",\"title\":\"Charge cards\"}},{{\"id\":\"B-2\",\"title\":\"Refunds\"}}]}}'\nelse\n  printf '{{\"schema_version\":1,\"tasks\":%s}}' \"$(cat {all})\"\nfi\n",
            log = fixture.path("calls.log").display(),
            all = fixture.path("all.json").display(),
        ),
    );
    fixture.script(
        "get.sh",
        &format!(
            "printf 'get %s\\n' \"$HP_TASK_ID\" >> {log}\nif [ \"$HP_TASK_ID\" = OLD-9 ]; then\n  printf '{{\"schema_version\":1,\"task\":{{\"id\":\"OLD-9\",\"title\":\"An old one\"}}}}'\nelse\n  printf '{{\"schema_version\":1,\"task\":null}}'\nfi\n",
            log = fixture.path("calls.log").display(),
        ),
    );
    let get = if with_get {
        format!("get = [\"{}\"]\n", fixture.path("get.sh").display())
    } else {
        String::new()
    };
    fixture.config(&format!(
        "[tracker.\"{}\"]\nlist = [\"{}\"]\n{get}",
        fixture.root.join("acme").display(),
        fixture.path("list.sh").display(),
    ));
    assert!(fixture.run(&["new", "acme"]).status.success());
    fixture.node(
        "t-0001",
        "Billing",
        "root",
        "coordinator",
        "task_scope = \"billing\"\n",
    );
    fixture.node(
        "t-0002",
        "Card worker",
        "t-0001",
        "worker",
        "tasks = [\"B-1\"]\n",
    );
    fixture
}

#[test]
fn a_coordinator_with_a_task_scope_owns_what_the_tracker_lists_for_it() {
    let fixture = tracked(false);
    let view = fixture.json(&["task", "list", "acme", "--node", "t-0001", "--json"]);
    assert_eq!(titles(&view["tasks"]), ["Charge cards", "Refunds"]);
    assert_eq!(view["tasks"][0]["id"], "B-1");
    assert_eq!(view["tasks"][0]["assignees"][0]["id"], "t-0002");
    assert!(
        fixture
            .calls()
            .contains(&"list t-0001 [billing]".to_string()),
        "{:?}",
        fixture.calls()
    );
    let project = fixture.json(&["task", "list", "acme", "--json"]);
    // The root's list is the whole project, as before.
    assert_eq!(
        titles(&project["tasks"]),
        ["Charge cards", "Refunds", "Hire"]
    );
    // In the project view a task is its scoped coordinator's.
    assert_eq!(titles(&project["owners"][0]["tasks"]), ["Hire"]);
    assert_eq!(
        titles(&project["owners"][1]["tasks"]),
        ["Charge cards", "Refunds"]
    );
    assert!(fixture.calls().contains(&"list root []".to_string()));
    // Each scope has its own cache: a second look asks nobody.
    let before = fixture.calls().len();
    fixture.json(&["task", "list", "acme", "--json"]);
    assert_eq!(fixture.calls().len(), before);

    // `node set --task-scope` changes the scope; the next list asks for it.
    assert!(
        fixture
            .run(&["node", "set", "acme", "t-0001", "--task-scope", "general"])
            .status
            .success()
    );
    fixture.json(&["task", "list", "acme", "--node", "t-0001", "--json"]);
    assert!(
        fixture
            .calls()
            .contains(&"list t-0001 [general]".to_string())
    );
}

/// A task the lists leave out is found by its id with `get`: an assignment
/// to it is accepted, the coordinator's view shows it, and an ask that links
/// it reads its state instead of `missing`.
#[test]
fn a_task_no_list_prints_is_found_by_its_id() {
    let fixture = tracked(true);
    assert!(
        fixture
            .run(&["node", "assign", "acme", "t-0001", "--task", "OLD-9"])
            .status
            .success()
    );
    let refused = fixture.run(&["node", "assign", "acme", "t-0001", "--task", "NOPE-1"]);
    assert!(!refused.status.success());
    let view = fixture.json(&["task", "list", "acme", "--node", "t-0001", "--json"]);
    let last = view["tasks"].as_array().unwrap().last().unwrap().clone();
    assert_eq!(
        (last["id"].clone(), last["title"].clone()),
        (json!("OLD-9"), json!("An old one"))
    );
    assert_eq!(last["via"], "assigned");

    fixture.write(
        "STATUS.md",
        "# Status\n## Asked\n- [working] The old one : on it (task:OLD-9)\n",
    );
    fixture.text(&["context", "acme", "--peek"]);
    let asks = fixture.json(&["asks", "--json"]);
    assert_eq!(asks["asks"][0]["links"][0]["state"], "todo");
    assert_eq!(asks["asks"][0]["links"][0]["title"], "An old one");
}

//! The records a person reads later: each thread's report, task file (with
//! its follow-ups) and extra Next lines, its library, and the status files of
//! the root coordinator and of coordinator nodes. They live in the project
//! folder unless `records` in PROJECT.md names another folder, such as a
//! synced notes vault. Everything else (settings, memory, `TASKS.md`,
//! routines, node scopes, thread state, the inbox, scratch files) stays in
//! the project folder: internal state never goes to a synced folder.
//!
//! Every reader and writer of a record builds its path here, from
//! [`Project::records_dir`].

use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

use crate::paths::Ctx;
use crate::project::Project;

/// The home copy of a thread's report.
pub fn report(records: &Path, id: &str) -> PathBuf {
    records.join("threads").join(format!("{id}.md"))
}

/// A thread's task and every prompt forwarded to it (`## Follow-ups`).
pub fn task(records: &Path, id: &str) -> PathBuf {
    records.join("threads").join(format!("{id}.task.md"))
}

/// The Next lines the coordinator added to a thread's list.
pub fn next(records: &Path, id: &str) -> PathBuf {
    records.join("threads").join(format!("{id}.next.md"))
}

/// The home copy of the files a thread produced.
pub fn library(records: &Path, id: &str) -> PathBuf {
    records.join("library").join(id)
}

/// The root coordinator's status file.
pub fn root_status(records: &Path) -> PathBuf {
    records.join(crate::status_file::NAME)
}

/// A coordinator node's status file.
pub fn node_status(records: &Path, id: &str) -> PathBuf {
    records
        .join("nodes")
        .join(id)
        .join(crate::status_file::NAME)
}

/// The folder `records` names in PROJECT.md: empty, or a value that is not
/// an absolute path, is the project folder. `~/` is the user's home, for a
/// hand edit; `set` stores the expanded path.
pub fn resolve(project_dir: &Path, value: &str) -> PathBuf {
    let _ = value;
    project_dir.to_path_buf()
}

/// `set <slug> records <folder>`: validates the folder, moves the existing
/// records there and writes the setting, all under the project lock. An
/// empty value brings them back to the project folder.
pub fn set(ctx: &Ctx, project: &Project, value: &str) -> Result<String> {
    let _ = (ctx, project, value);
    bail!("records cannot be set yet")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::Env;
    use crate::runner::fake::FakeRunner;

    struct Setup {
        home: tempfile::TempDir,
        env: Env,
        runner: FakeRunner,
    }

    impl Setup {
        fn new() -> Setup {
            let home = tempfile::tempdir().unwrap();
            let env = Env::for_test(home.path(), &[]);
            Setup {
                home,
                env,
                runner: FakeRunner::new(),
            }
        }

        fn ctx(&self) -> Ctx<'_> {
            Ctx {
                env: &self.env,
                root: self.home.path().join("root"),
                config_dir: self.home.path().join("cfg"),
                runner: &self.runner,
                detached_ticker: false,
            }
        }

        fn project(&self, slug: &str) -> Project {
            crate::project::create(&self.home.path().join("root"), slug, "", vec![]).unwrap()
        }

        fn vault(&self) -> PathBuf {
            self.home.path().join("vault").join("demo")
        }
    }

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn read(path: &Path) -> String {
        std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
    }

    fn records_setting(project: &Project) -> String {
        project.read_project_md().unwrap().0.records
    }

    /// A project that never sets `records` keeps every record where it
    /// always was, and `new` writes the same PROJECT.md keys as before.
    #[test]
    fn a_project_without_records_keeps_every_record_in_its_folder() {
        let setup = Setup::new();
        let project = setup.project("demo");
        let dir = project.dir();
        assert_eq!(project.records_dir(), dir);
        assert_eq!(
            crate::thread::home_report_path(&project, "t-0001"),
            dir.join("threads/t-0001.md")
        );
        assert_eq!(
            crate::thread::task_path(&project, "t-0001"),
            dir.join("threads/t-0001.task.md")
        );
        assert_eq!(
            crate::thread::extra_next_path(&project, "t-0001"),
            dir.join("threads/t-0001.next.md")
        );
        assert_eq!(
            crate::thread::home_library_path(&project, "t-0001"),
            dir.join("library/t-0001")
        );
        assert_eq!(crate::status_file::root(&project), dir.join("STATUS.md"));
        assert_eq!(
            crate::organizations::status_path(&project, "t-0002"),
            dir.join("nodes/t-0002/STATUS.md")
        );
        let text = read(&project.project_md());
        for key in ["records", "progress", "turn_reminder"] {
            assert!(!text.contains(key), "{key} in a new PROJECT.md:\n{text}");
        }
        let settings = project.read_project_md().unwrap().0;
        assert!(settings.progress && settings.turn_reminder);
    }

    #[test]
    fn records_names_where_every_record_lives() {
        let setup = Setup::new();
        let project = setup.project("demo");
        let vault = setup.vault();
        std::fs::create_dir_all(&vault).unwrap();
        let text = read(&project.project_md()).replacen(
            "+++\n",
            &format!("+++\nrecords = \"{}\"\n", vault.display()),
            1,
        );
        std::fs::write(project.project_md(), text).unwrap();
        assert_eq!(project.records_dir(), vault);
        assert_eq!(
            crate::thread::home_report_path(&project, "t-0001"),
            vault.join("threads/t-0001.md")
        );
        assert_eq!(
            crate::thread::task_path(&project, "t-0001"),
            vault.join("threads/t-0001.task.md")
        );
        assert_eq!(
            crate::thread::extra_next_path(&project, "t-0001"),
            vault.join("threads/t-0001.next.md")
        );
        assert_eq!(
            crate::thread::home_library_path(&project, "t-0001"),
            vault.join("library/t-0001")
        );
        assert_eq!(crate::status_file::root(&project), vault.join("STATUS.md"));
        assert_eq!(
            crate::organizations::status_path(&project, "t-0002"),
            vault.join("nodes/t-0002/STATUS.md")
        );
        // The thread record and the node's scope stay in the project folder.
        assert_eq!(
            crate::thread::record_path(&project, "t-0001"),
            project.dir().join("threads/t-0001.toml")
        );
        assert_eq!(
            crate::organizations::node_scope_dir(&project, "t-0002"),
            project.dir().join("nodes/t-0002")
        );
    }

    #[test]
    fn a_hand_written_value_resolves_to_an_absolute_folder_or_the_project() {
        let dir = Path::new("/r/demo");
        assert_eq!(resolve(dir, ""), dir);
        assert_eq!(resolve(dir, "  "), dir);
        assert_eq!(resolve(dir, "relative/x"), dir);
        assert_eq!(resolve(dir, "/vault/demo"), Path::new("/vault/demo"));
        assert_eq!(resolve(dir, "/vault/demo/"), Path::new("/vault/demo"));
    }

    /// The fixture every move test starts from: one of each record in the
    /// project folder, beside files that must stay.
    fn records_in(project: &Project) {
        let dir = project.dir();
        write(&dir.join("threads/t-0001.md"), "Status: done\n");
        write(&dir.join("threads/t-0001.task.md"), "The task.\n");
        write(&dir.join("threads/t-0001.next.md"), "- Ship it\n");
        write(
            &dir.join("threads/t-0001/notes.txt"),
            "a tab thread's folder",
        );
        write(&dir.join("library/t-0001/deep/out.html"), "<p>hi</p>");
        write(&dir.join("STATUS.md"), "# Status\nObjective: root\n");
        write(
            &dir.join("nodes/t-0002/STATUS.md"),
            "# Status\nObjective: node\n",
        );
        write(
            &dir.join("nodes/t-0002/INSTRUCTIONS.md"),
            "# Node instructions\n",
        );
        let mtime =
            std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        std::fs::File::options()
            .write(true)
            .open(dir.join("STATUS.md"))
            .unwrap()
            .set_modified(mtime)
            .unwrap();
    }

    const MOVED: [&str; 6] = [
        "threads/t-0001.md",
        "threads/t-0001.task.md",
        "threads/t-0001.next.md",
        "library/t-0001/deep/out.html",
        "STATUS.md",
        "nodes/t-0002/STATUS.md",
    ];
    const KEPT: [&str; 4] = [
        "threads/t-0001/notes.txt",
        "nodes/t-0002/INSTRUCTIONS.md",
        "PROJECT.md",
        "TASKS.md",
    ];

    #[test]
    fn set_records_moves_the_records_there_and_back_and_is_idempotent() {
        let setup = Setup::new();
        let project = setup.project("demo");
        crate::thread::allocate(&project, |_| {}).unwrap();
        records_in(&project);
        let dir = project.dir();
        let vault = setup.vault();
        let before = std::fs::metadata(dir.join("STATUS.md"))
            .unwrap()
            .modified()
            .unwrap();

        set(&setup.ctx(), &project, &vault.to_string_lossy()).unwrap();
        assert_eq!(records_setting(&project), vault.to_string_lossy());
        for path in MOVED {
            assert!(vault.join(path).is_file(), "{path} was not moved");
            assert!(!dir.join(path).exists(), "{path} was left behind");
        }
        for path in KEPT {
            assert!(dir.join(path).is_file(), "{path} must stay");
        }
        assert!(dir.join("threads/t-0001.toml").is_file());
        assert!(!dir.join("library/t-0001").exists());
        assert_eq!(
            read(&vault.join("STATUS.md")),
            "# Status\nObjective: root\n"
        );
        assert_eq!(
            std::fs::metadata(vault.join("STATUS.md"))
                .unwrap()
                .modified()
                .unwrap(),
            before,
            "a status file keeps its time, which `status-stale` compares"
        );
        // AGENTS.md names the status file where it now is.
        assert!(
            read(&dir.join("AGENTS.md")).contains(&vault.join("STATUS.md").display().to_string())
        );

        // Again: nothing changes.
        set(&setup.ctx(), &project, &vault.to_string_lossy()).unwrap();
        for path in MOVED {
            assert!(vault.join(path).is_file(), "{path}");
        }

        // Empty brings them home by the same rule.
        set(&setup.ctx(), &project, "").unwrap();
        assert_eq!(records_setting(&project), "");
        for path in MOVED {
            assert!(dir.join(path).is_file(), "{path} did not come back");
            assert!(!vault.join(path).exists(), "{path} stayed in the vault");
        }
        assert!(
            read(&project.project_md())
                .lines()
                .all(|l| !l.starts_with("records"))
        );
    }

    #[test]
    fn a_different_file_in_the_way_moves_nothing() {
        let setup = Setup::new();
        let project = setup.project("demo");
        records_in(&project);
        let vault = setup.vault();
        write(&vault.join("threads/t-0001.task.md"), "Another task.\n");
        // The same content already there is not a conflict.
        write(&vault.join("threads/t-0001.md"), "Status: done\n");

        let error = set(&setup.ctx(), &project, &vault.to_string_lossy()).unwrap_err();
        let error = format!("{error:#}");
        assert!(error.contains("threads/t-0001.task.md"), "{error}");
        assert_eq!(records_setting(&project), "");
        for path in MOVED {
            assert!(project.dir().join(path).is_file(), "{path} was touched");
        }
        assert_eq!(
            read(&vault.join("threads/t-0001.task.md")),
            "Another task.\n"
        );
        assert!(!vault.join("STATUS.md").exists(), "nothing was copied");

        std::fs::remove_file(vault.join("threads/t-0001.task.md")).unwrap();
        set(&setup.ctx(), &project, &vault.to_string_lossy()).unwrap();
        assert_eq!(read(&vault.join("threads/t-0001.md")), "Status: done\n");
        assert!(!project.dir().join("threads/t-0001.md").exists());
    }

    #[test]
    fn the_folder_must_be_absolute_real_and_outside_every_project() {
        let setup = Setup::new();
        let ctx = setup.ctx();
        let project = setup.project("demo");
        let other = setup.project("other");
        let refused = |value: &str, why: &str| {
            let error = format!("{:#}", set(&ctx, &project, value).unwrap_err());
            assert!(error.contains(why), "{value}: {error}");
            assert_eq!(records_setting(&project), "", "{value}");
        };
        refused("vault/demo", "absolute");
        refused(&format!("{}/../x", setup.home.path().display()), "..");
        refused(&ctx.root.join("notes").to_string_lossy(), "projects root");
        let link = setup.home.path().join("link");
        std::fs::create_dir_all(setup.home.path().join("target")).unwrap();
        std::os::unix::fs::symlink(setup.home.path().join("target"), &link).unwrap();
        refused(&link.to_string_lossy(), "symbolic link");
        let file = setup.home.path().join("file");
        std::fs::write(&file, "").unwrap();
        refused(&file.to_string_lossy(), "not a folder");

        let shared = setup.home.path().join("vault/shared");
        set(&ctx, &other, &shared.to_string_lossy()).unwrap();
        refused(&shared.to_string_lossy(), "other");
        refused(&shared.join("sub").to_string_lossy(), "other");

        // `~/` is the home of the user running `set`, and the folder is made.
        set(&ctx, &project, "~/notes/demo").unwrap();
        let made = setup.home.path().join("notes/demo");
        assert_eq!(records_setting(&project), made.to_string_lossy());
        assert!(made.join("threads").is_dir() && made.join("library").is_dir());
    }
}

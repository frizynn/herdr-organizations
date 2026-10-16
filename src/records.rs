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

use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::paths::Ctx;
use crate::project::{Project, write_atomic};

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

/// A record's path as a message names it: relative to the project folder
/// when it is inside it (`threads/t-0001.md`), else absolute.
pub fn shown(project: &Project, path: &Path) -> String {
    path.strip_prefix(project.dir())
        .unwrap_or(path)
        .display()
        .to_string()
}

/// The folder `records` names in PROJECT.md: empty, or a value that is not
/// an absolute path, is the project folder. `~/` is the user's home, for a
/// hand edit; `set` stores the expanded path.
pub fn resolve(project_dir: &Path, value: &str) -> PathBuf {
    let value = value.trim();
    let path = match value.strip_prefix("~/") {
        Some(rest) => std::env::var_os("HOME")
            .map_or_else(PathBuf::new, |home| PathBuf::from(home).join(rest)),
        None => PathBuf::from(value),
    };
    if path.is_absolute() {
        path.components().collect()
    } else {
        project_dir.to_path_buf()
    }
}

/// `set <slug> records <folder>`: validates the folder, moves the existing
/// records there and writes the setting, all under the project lock. An
/// empty value brings them back to the project folder. Every file is copied
/// first, keeping its modification time, and the originals are removed
/// only once all copies and the setting are written; a different file
/// already at a destination moves nothing. Says what it moved.
pub fn set(ctx: &Ctx, project: &Project, value: &str) -> Result<String> {
    let wanted = folder(ctx, project, value)?;
    let stored = wanted
        .as_ref()
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_default();
    let _lock = project.lock()?;
    let from = project.records_dir();
    let to = wanted.unwrap_or_else(|| project.dir());
    let text = std::fs::read_to_string(project.project_md())?;
    let edited = crate::settings::set_in(&text, "records", &stored)?;
    let moves = plan(&from, &to)?;
    let created = copy_all(&moves)?;
    if let Err(error) = write_atomic(&project.project_md(), edited.as_bytes()) {
        undo(&created);
        return Err(error);
    }
    remove_originals(&moves, &from, &project.dir());
    Ok(if moves.is_empty() {
        format!("records are in {}", to.display())
    } else {
        format!(
            "moved {} record files from {} to {}",
            moves.len(),
            from.display(),
            to.display()
        )
    })
}

/// The folder `set` would store: `None` for the project folder. It must be
/// absolute (`~/` expands to the caller's home), without `..`, not a
/// symbolic link or a file, outside the projects root, and outside every
/// project and every other project's records folder. It is made, with its
/// `threads/` and `library/`, when missing.
fn folder(ctx: &Ctx, project: &Project, value: &str) -> Result<Option<PathBuf>> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    let path = ctx.env.expand_tilde(value);
    if !path.is_absolute() {
        bail!("records must be an absolute folder or start with `~/`, not `{value}`");
    }
    if path.components().any(|c| c == Component::ParentDir) {
        bail!("records may not contain `..`: `{value}`");
    }
    let path: PathBuf = path.components().collect();
    match std::fs::symlink_metadata(&path) {
        Ok(meta) if meta.file_type().is_symlink() => bail!(
            "{} is a symbolic link; name the folder it points to",
            path.display()
        ),
        Ok(meta) if !meta.is_dir() => bail!("{} is not a folder", path.display()),
        _ => {}
    }
    let real = real_path(&path);
    let root = real_path(&ctx.root);
    if real.starts_with(&root) {
        bail!(
            "{} is inside the projects root {}; records go outside it",
            path.display(),
            ctx.root.display()
        );
    }
    for slug in crate::project::list_slugs(&ctx.root) {
        let Ok(other) = Project::load(&ctx.root, &slug) else {
            continue;
        };
        if real.starts_with(other.canonical_dir()) {
            bail!("{} is inside project `{slug}`", path.display());
        }
        let theirs = other.records_dir();
        if slug != project.slug && theirs != other.dir() && real.starts_with(real_path(&theirs)) {
            bail!(
                "{} is inside the records folder of project `{slug}` ({})",
                path.display(),
                theirs.display()
            );
        }
    }
    for sub in ["threads", "library"] {
        std::fs::create_dir_all(path.join(sub))
            .with_context(|| format!("could not create {}", path.join(sub).display()))?;
    }
    Ok(Some(path))
}

/// `path` with its longest existing ancestor's symbolic links resolved, so
/// a folder that is not made yet still compares with real ones.
fn real_path(path: &Path) -> PathBuf {
    let mut missing = Vec::new();
    let mut current = path;
    loop {
        if let Ok(real) = std::fs::canonicalize(current) {
            return missing
                .iter()
                .rev()
                .fold(real, |path, name| path.join(name));
        }
        match (current.parent(), current.file_name()) {
            (Some(parent), Some(name)) => {
                missing.push(name.to_os_string());
                current = parent;
            }
            _ => return path.to_path_buf(),
        }
    }
}

/// One record file to move; `copy` is false when the same bytes are
/// already at `to`.
struct Move {
    from: PathBuf,
    to: PathBuf,
    copy: bool,
}

/// What moving every record from one folder to another takes. Refuses,
/// before anything changes, a record that is not a regular file, a
/// different file already at a destination, and a destination inside the
/// current folder.
fn plan(from: &Path, to: &Path) -> Result<Vec<Move>> {
    let (real_from, real_to) = (real_path(from), real_path(to));
    if real_from == real_to {
        return Ok(Vec::new());
    }
    if real_to.starts_with(&real_from) {
        bail!(
            "{} is inside the current records folder {}",
            to.display(),
            from.display()
        );
    }
    let mut moves = Vec::new();
    let mut conflicts = Vec::new();
    for relative in record_files(from)? {
        let (source, target) = (from.join(&relative), to.join(&relative));
        let copy = match std::fs::symlink_metadata(&target) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
            Ok(meta) if meta.is_file() && std::fs::read(&target)? == std::fs::read(&source)? => {
                false
            }
            Ok(_) => {
                conflicts.push(relative.display().to_string());
                continue;
            }
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("could not inspect {}", target.display()));
            }
        };
        moves.push(Move {
            from: source,
            to: target,
            copy,
        });
    }
    if !conflicts.is_empty() {
        bail!(
            "{} already holds a different {}; nothing was moved. Move it away, then run `set` again",
            to.display(),
            conflicts.join(", ")
        );
    }
    Ok(moves)
}

/// The record files in a records folder, relative to it: reports, task
/// files and Next lines in `threads/`, every file under `library/<id>/`,
/// `STATUS.md` and `nodes/<id>/STATUS.md`. Anything else there (thread
/// state, tab threads' folders, node scopes) is not a record.
fn record_files(records: &Path) -> Result<Vec<PathBuf>> {
    let is_id = |name: &str| crate::thread::validate_id(name).is_ok();
    let mut files = Vec::new();
    for name in names(&records.join("threads")) {
        let id = [".task.md", ".next.md", ".md"]
            .iter()
            .find_map(|suffix| name.strip_suffix(suffix));
        if id.is_some_and(is_id) {
            files.push(Path::new("threads").join(name));
        }
    }
    for name in names(&records.join("library"))
        .into_iter()
        .filter(|n| is_id(n))
    {
        walk(records, &Path::new("library").join(name), &mut files);
    }
    let status = PathBuf::from(crate::status_file::NAME);
    let nodes = names(&records.join("nodes"))
        .into_iter()
        .filter(|n| is_id(n))
        .map(|id| Path::new("nodes").join(id).join(&status));
    files.extend(
        std::iter::once(status.clone())
            .chain(nodes)
            .filter(|path| std::fs::symlink_metadata(records.join(path)).is_ok()),
    );
    for relative in &files {
        if !std::fs::symlink_metadata(records.join(relative))?.is_file() {
            bail!(
                "{} is not a regular file; move it by hand, then run `set` again",
                records.join(relative).display()
            );
        }
    }
    Ok(files)
}

fn names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|e| e.file_name().into_string().ok())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

/// Every file under `relative`, a link included so the caller refuses it.
fn walk(base: &Path, relative: &Path, files: &mut Vec<PathBuf>) {
    let is_dir = std::fs::symlink_metadata(base.join(relative)).is_ok_and(|m| m.is_dir());
    if !is_dir {
        files.push(relative.to_path_buf());
        return;
    }
    for name in names(&base.join(relative)) {
        walk(base, &relative.join(name), files);
    }
}

/// Copies every move that needs it, keeping modification times, and
/// returns the files and folders it made. On a failure it removes them
/// again and nothing changed.
fn copy_all(moves: &[Move]) -> Result<Vec<PathBuf>> {
    let mut created = Vec::new();
    for m in moves.iter().filter(|m| m.copy) {
        let copied = (|| -> Result<()> {
            make_dirs(
                m.to.parent().context("a record has no folder")?,
                &mut created,
            )?;
            let modified = std::fs::metadata(&m.from)?.modified()?;
            write_atomic(&m.to, &std::fs::read(&m.from)?)?;
            created.push(m.to.clone());
            std::fs::File::options()
                .write(true)
                .open(&m.to)?
                .set_modified(modified)?;
            Ok(())
        })();
        if let Err(error) = copied {
            undo(&created);
            return Err(error.context(format!(
                "could not copy {}; nothing was moved",
                m.from.display()
            )));
        }
    }
    Ok(created)
}

fn make_dirs(dir: &Path, created: &mut Vec<PathBuf>) -> Result<()> {
    if dir.is_dir() {
        return Ok(());
    }
    make_dirs(dir.parent().context("no folder to make it in")?, created)?;
    std::fs::create_dir(dir).with_context(|| format!("could not create {}", dir.display()))?;
    created.push(dir.to_path_buf());
    Ok(())
}

/// Removes what `copy_all` made, newest first.
fn undo(created: &[PathBuf]) {
    for path in created.iter().rev() {
        let _ = std::fs::remove_file(path).or_else(|_| std::fs::remove_dir(path));
    }
}

/// Removes the originals once their copies and the setting are written, and
/// the folders that leaves empty, except the project folder's own
/// `threads/`, `library/` and `nodes/`.
fn remove_originals(moves: &[Move], from: &Path, project_dir: &Path) {
    let kept = |dir: &Path| {
        from == project_dir
            && dir.strip_prefix(from).is_ok_and(|relative| {
                relative == Path::new("threads")
                    || relative == Path::new("library")
                    || relative.starts_with("nodes")
            })
    };
    for m in moves {
        let _ = std::fs::remove_file(&m.from);
        let mut dir = m.from.parent();
        while let Some(current) = dir {
            if current == from || kept(current) || std::fs::remove_dir(current).is_err() {
                break;
            }
            dir = current.parent();
        }
    }
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

    /// `set <slug> records <value>`, as the CLI and the popup run it.
    fn set_records(ctx: &Ctx, project: &Project, value: &str) -> Result<()> {
        crate::settings::set(ctx, &project.slug, "records", value)
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

        set_records(&setup.ctx(), &project, &vault.to_string_lossy()).unwrap();
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
        set_records(&setup.ctx(), &project, &vault.to_string_lossy()).unwrap();
        for path in MOVED {
            assert!(vault.join(path).is_file(), "{path}");
        }

        // Empty brings them home by the same rule.
        set_records(&setup.ctx(), &project, "").unwrap();
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

        let error = set_records(&setup.ctx(), &project, &vault.to_string_lossy()).unwrap_err();
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
        set_records(&setup.ctx(), &project, &vault.to_string_lossy()).unwrap();
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
            let error = format!("{:#}", set_records(&ctx, &project, value).unwrap_err());
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
        set_records(&ctx, &other, &shared.to_string_lossy()).unwrap();
        refused(&shared.to_string_lossy(), "other");
        refused(&shared.join("sub").to_string_lossy(), "other");

        // `~/` is the home of the user running `set`, and the folder is made.
        set_records(&ctx, &project, "~/notes/demo").unwrap();
        let made = setup.home.path().join("notes/demo");
        assert_eq!(records_setting(&project), made.to_string_lossy());
        assert!(made.join("threads").is_dir() && made.join("library").is_dir());
    }
}

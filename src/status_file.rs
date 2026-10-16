//! The status file: one plain Markdown file a coordinator writes for whoever
//! wants to know how its work stands (the user, the root, a sibling), read
//! when they want instead of being sent. The root keeps the project's
//! `STATUS.md`; a spawning coordinator node keeps `nodes/<id>/STATUS.md`.
//!
//! It also keeps, per coordinator, the time of the user's last two prompts
//! (`.state/asked.json`, never their text), so a file older than the previous
//! one can be named: the coordinator answered without updating it.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::project::Project;

pub const NAME: &str = "STATUS.md";
/// The size limit of every status file, the root's and a node's: what the
/// skills ask for, what `context` prints, what `--json` carries and the most
/// a node's brief inlines.
pub const MAX_CHARS: usize = 4_000;
/// What `new` writes: the skill's template with nothing filled in.
pub const TEMPLATE: &str =
    "# Status\nObjective: (none)\n## Asked\n## Needs your decision\n## Decisions\n";
/// The root coordinator's key in `.state/asked.json`; nodes use their id.
pub const ROOT_KEY: &str = "root";
/// What a coordinator node's hook adds when its file is stale.
pub const NODE_STALE: &str =
    "Your STATUS.md is older than the user's previous message. Update it before you answer.";

/// The root coordinator's status file.
pub fn root(project: &Project) -> PathBuf {
    crate::records::root_status(&project.records_dir())
}

/// The root's status file, empty when it is missing.
pub fn root_text(project: &Project) -> String {
    std::fs::read_to_string(root(project)).unwrap_or_default()
}

/// A status file in the `--json` contract (docs/json.md).
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct StatusJson {
    pub path: String,
    /// The first `MAX_CHARS` characters.
    pub text: String,
    /// RFC 3339 modification time.
    pub updated: String,
}

/// The file as `--json` and the popup show it: a regular file opened without
/// following a symbolic link, cut to its first `MAX_CHARS` characters.
/// `None` when it is missing or is not a regular file.
pub fn read(path: &Path) -> Option<StatusJson> {
    if !std::fs::symlink_metadata(path).ok()?.is_file() {
        return None;
    }
    let file = crate::organizations::open_regular_file_no_follow(path).ok()?;
    let updated = file.metadata().ok()?.modified().ok();
    let mut bytes = Vec::new();
    // A character is at most four bytes.
    file.take((MAX_CHARS * 4) as u64)
        .read_to_end(&mut bytes)
        .ok()?;
    Some(StatusJson {
        path: path.to_string_lossy().into_owned(),
        text: String::from_utf8_lossy(&bytes)
            .chars()
            .take(MAX_CHARS)
            .collect(),
        updated: updated.map(rfc3339).unwrap_or_default(),
    })
}

/// RFC 3339 to the second.
pub fn rfc3339(time: std::time::SystemTime) -> String {
    jiff::Timestamp::try_from(time)
        .ok()
        .and_then(|t| t.round(jiff::Unit::Second).ok())
        .map(|t| t.to_string())
        .unwrap_or_default()
}

/// Unix seconds of the user's last two prompts to one coordinator; 0 for none.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
struct Asked {
    previous: i64,
    last: i64,
}

fn asked_path(project: &Project) -> PathBuf {
    project.state_dir().join("asked.json")
}

/// Records that the user wrote to the coordinator `key` at `now` and returns
/// the time of the message before it, if there was one.
pub fn record_ask(project: &Project, key: &str, now: i64) -> Result<Option<i64>> {
    let _lock = project.lock()?;
    let path = asked_path(project);
    let mut all: BTreeMap<String, Asked> = crate::project::read_json(&path).unwrap_or_default();
    let entry = all.entry(key.to_string()).or_default();
    entry.previous = entry.last;
    entry.last = now;
    let previous = entry.previous;
    crate::project::write_json(&path, &all)?;
    Ok((previous > 0).then_some(previous))
}

/// The time of the user's message before their last one to `key`.
pub fn previous_ask(project: &Project, key: &str) -> Option<i64> {
    let all: BTreeMap<String, Asked> = crate::project::read_json(&asked_path(project))?;
    all.get(key)
        .map(|a| a.previous)
        .filter(|previous| *previous > 0)
}

/// The file was last written before `previous` (a missing one counts too):
/// the coordinator answered that message without updating it.
pub fn stale(path: &Path, previous: Option<i64>) -> bool {
    let Some(previous) = previous else {
        return false;
    };
    let written = std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| jiff::Timestamp::try_from(t).ok())
        .map(|t| t.as_second());
    written.is_none_or(|written| written < previous)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asks_keep_only_the_last_two_times_per_coordinator() {
        let home = tempfile::tempdir().unwrap();
        let project = crate::project::create(home.path(), "demo", "", vec![]).unwrap();
        assert_eq!(record_ask(&project, ROOT_KEY, 100).unwrap(), None);
        assert_eq!(previous_ask(&project, ROOT_KEY), None);
        assert_eq!(record_ask(&project, ROOT_KEY, 200).unwrap(), Some(100));
        assert_eq!(record_ask(&project, "t-0001", 250).unwrap(), None);
        assert_eq!(record_ask(&project, ROOT_KEY, 300).unwrap(), Some(200));
        assert_eq!(previous_ask(&project, ROOT_KEY), Some(200));
        assert_eq!(previous_ask(&project, "t-0001"), None);
    }

    /// Both coordinator skills show the shape `new` writes and its limit.
    #[test]
    fn the_skills_show_the_template_and_the_limit() {
        let limit = format!(
            "under {},{:03} characters",
            MAX_CHARS / 1000,
            MAX_CHARS % 1000
        );
        let headings = |text: &str| -> Vec<String> {
            text.lines()
                .filter(|line| line.starts_with('#') || line.starts_with("Objective:"))
                .map(|line| line.split(':').next().unwrap().to_string())
                .collect()
        };
        for skill in [
            include_str!("../skill/COORDINATOR.md"),
            include_str!("../skill/COORDINATOR_NODE.md"),
        ] {
            let block = skill.split_once("```\n# Status\n").unwrap().1;
            let block = format!("# Status\n{}", block.split_once("```").unwrap().0);
            assert_eq!(headings(&block), headings(TEMPLATE));
            assert!(skill.contains(&limit), "{limit}");
        }
    }

    #[test]
    fn a_file_is_stale_only_when_written_before_the_previous_message() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join(NAME);
        assert!(!stale(&path, None), "no previous message");
        assert!(stale(&path, Some(1)), "never written");
        std::fs::write(&path, "# Status\n").unwrap();
        let now = jiff::Timestamp::now().as_second();
        assert!(!stale(&path, Some(now - 60)));
        assert!(stale(&path, Some(now + 60)));
    }
}

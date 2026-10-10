//! `sidebar install`: adds the rows that render the organization tokens to
//! Herdr's own sidebar layout. It only appends rows (and `state_text` when no
//! row shows the state), never edits or removes the user's rows, and never
//! touches the panel headers, which Herdr draws outside `rows`.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use toml_edit::{Array, DocumentMut, InlineTable, Item, Table, Value};

use crate::paths::Env;

/// Herdr's documented row limit per layout.
const MAX_ROWS: usize = 16;
pub const RED: &str = "#f38ba8";
pub const YELLOW: &str = "#f9e2af";
pub const BLUE: &str = "#89b4fa";
pub const GREEN: &str = "#a6e3a1";

pub fn default_path(env: &Env) -> PathBuf {
    env.var("HERDR_CONFIG_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| env.expand_tilde("~/.config/herdr/config.toml"))
}

fn styled(token: &str, fg: &str) -> InlineTable {
    let mut t = InlineTable::new();
    t.insert("token", token.into());
    t.insert("fg", fg.into());
    t
}

fn rule(starts_with: &str, fg: &str) -> InlineTable {
    let mut r = InlineTable::new();
    r.insert("starts_with", starts_with.into());
    r.insert("fg", fg.into());
    r.insert("dim", false.into());
    r
}

/// `$org_need $org_work $org_review`, one colour each; empty ones vanish.
fn space_row() -> Value {
    let mut row = Array::new();
    for (token, fg) in [
        ("$org_need", RED),
        ("$org_work", YELLOW),
        ("$org_review", BLUE),
    ] {
        row.push(styled(token, fg));
    }
    row.into()
}

/// `$org_task`, dimmed, blue for review and approved, green once merged.
fn agent_row() -> Value {
    let mut rules = Array::new();
    for (prefix, fg) in [("review", BLUE), ("approved", BLUE), ("merged", GREEN)] {
        rules.push(rule(prefix, fg));
    }
    let mut task = InlineTable::new();
    task.insert("token", "$org_task".into());
    task.insert("dim", true.into());
    task.insert("rules", rules.into());
    let mut row = Array::new();
    row.push(task);
    row.into()
}

fn rows_of(defaults: &[&[&str]]) -> Array {
    let mut rows = Array::new();
    for row in defaults {
        let mut r = Array::new();
        for token in *row {
            r.push(*token);
        }
        rows.push(r);
    }
    rows
}

fn mentions(rows: &Array, token: &str) -> bool {
    rows.iter().any(|row| {
        row.as_array().is_some_and(|r| {
            r.iter().any(|v| match v {
                Value::String(s) => s.value() == token,
                Value::InlineTable(t) => t.get("token").and_then(Value::as_str) == Some(token),
                _ => false,
            })
        })
    })
}

fn layout<'d>(doc: &'d mut DocumentMut, panel: &str) -> Result<&'d mut Table> {
    let ui = doc
        .entry("ui")
        .or_insert(Item::Table(Table::new()))
        .as_table_mut()
        .context("`ui` in Herdr's config is not a table")?;
    ui.set_implicit(true);
    let sidebar = ui
        .entry("sidebar")
        .or_insert(Item::Table(Table::new()))
        .as_table_mut()
        .context("`ui.sidebar` is not a table")?;
    sidebar.set_implicit(true);
    sidebar
        .entry(panel)
        .or_insert(Item::Table(Table::new()))
        .as_table_mut()
        .with_context(|| format!("`ui.sidebar.{panel}` is not a table"))
}

/// Appends `row` unless `token` is already shown. Returns whether it changed.
fn append(rows: &mut Array, row: &Value, token: &str, panel: &str) -> Result<bool> {
    if mentions(rows, token) {
        return Ok(false);
    }
    if rows.len() >= MAX_ROWS {
        bail!("ui.sidebar.{panel} already has {MAX_ROWS} rows, Herdr's limit; remove one first");
    }
    rows.push(row.clone());
    Ok(true)
}

/// The edited config text, or `None` when everything is already there.
pub fn plan(text: &str) -> Result<Option<String>> {
    let mut doc: DocumentMut = text.parse().context("Herdr's config.toml does not parse")?;
    let mut changed = false;

    let spaces = layout(&mut doc, "spaces")?;
    let rows = spaces
        .entry("rows")
        .or_insert(toml_edit::value(rows_of(&[
            &["state_icon", "workspace"],
            &["branch", "git_status"],
        ])))
        .as_array_mut()
        .context("ui.sidebar.spaces.rows is not an array")?;
    changed |= append(rows, &space_row(), "$org_need", "spaces")?;

    let agents = layout(&mut doc, "agents")?;
    let rows = agents
        .entry("rows")
        .or_insert(toml_edit::value(rows_of(&[
            &["state_icon", "machine", "workspace", "tab"],
            &["agent"],
        ])))
        .as_array_mut()
        .context("ui.sidebar.agents.rows is not an array")?;
    if !mentions(rows, "state_text")
        && let Some(first) = rows.get_mut(0).and_then(Value::as_array_mut)
        && first.len() < 16
    {
        // "needs you" is a state label; only `state_text` renders it.
        first.push("state_text");
        changed = true;
    }
    changed |= append(rows, &agent_row(), "$org_task", "agents")?;
    if let Some(overrides) = agents
        .get_mut("rows_by_agent")
        .and_then(Item::as_table_like_mut)
    {
        for (name, item) in overrides.iter_mut() {
            if let Some(rows) = item.as_array_mut() {
                changed |= append(
                    rows,
                    &agent_row(),
                    "$org_task",
                    &format!("agents.rows_by_agent.{name}"),
                )?;
            }
        }
    }
    Ok(changed.then(|| doc.to_string()))
}

/// Writes the plan with a dated backup of the previous file. Returns what it did.
pub fn install(path: &Path, dry_run: bool) -> Result<String> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => {
            return Err(error).with_context(|| format!("could not read {}", path.display()));
        }
    };
    let Some(edited) = plan(&text)? else {
        return Ok(format!(
            "{} already shows the organization tokens",
            path.display()
        ));
    };
    if dry_run {
        return Ok(edited);
    }
    if !text.is_empty() {
        let stamp = jiff::Timestamp::now().strftime("%Y%m%d-%H%M%S").to_string();
        let backup = path.with_extension(format!("toml.bak-organizations-{stamp}"));
        std::fs::write(&backup, &text)
            .with_context(|| format!("could not write {}", backup.display()))?;
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    crate::project::write_atomic(path, edited.as_bytes())?;
    Ok(format!(
        "added the organization rows to {}; reload Herdr's config (prefix then `reload config`) to see them",
        path.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_config_gets_herdrs_defaults_plus_our_rows() {
        let edited = plan("").unwrap().unwrap();
        let doc: toml::Value = toml::from_str(&edited).unwrap();
        let spaces = doc["ui"]["sidebar"]["spaces"]["rows"].as_array().unwrap();
        assert_eq!(spaces.len(), 3);
        assert_eq!(spaces[0].as_array().unwrap()[1].as_str(), Some("workspace"));
        let agents = doc["ui"]["sidebar"]["agents"]["rows"].as_array().unwrap();
        assert!(
            agents[0]
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v.as_str() == Some("state_text"))
        );
        assert_eq!(
            agents.last().unwrap()[0]["token"].as_str(),
            Some("$org_task")
        );
    }

    #[test]
    fn existing_rows_are_kept_and_only_appended_to() {
        let mine = r#"# my settings
[ui]
agent_panel_sort = "priority"

[ui.sidebar.agents]
rows = [["state_icon", { token = "agent", bold = false }, "state_text"], [{ token = "$hp_sub", dim = true }]]

[ui.sidebar.agents.rows_by_agent]
claude = [["agent"]]

[[keys.command]]
key = "prefix+a"
type = "plugin_action"
command = "herdr-projects.organizations"
"#;
        let edited = plan(mine).unwrap().unwrap();
        assert!(
            edited.starts_with("# my settings\n[ui]\nagent_panel_sort = \"priority\""),
            "{edited}"
        );
        assert!(edited.contains(r#"[["state_icon", { token = "agent", bold = false }, "state_text"], [{ token = "$hp_sub", dim = true }]"#), "{edited}");
        assert!(edited.contains("prefix+a"));
        let doc: toml::Value = toml::from_str(&edited).unwrap();
        let agents = doc["ui"]["sidebar"]["agents"]["rows"].as_array().unwrap();
        assert_eq!(
            agents.len(),
            3,
            "state_text was already there: nothing else changes"
        );
        let claude = doc["ui"]["sidebar"]["agents"]["rows_by_agent"]["claude"]
            .as_array()
            .unwrap();
        assert_eq!(claude.len(), 2);
    }

    #[test]
    fn installing_twice_changes_nothing_the_second_time() {
        let once = plan("").unwrap().unwrap();
        assert_eq!(plan(&once).unwrap(), None);
    }

    #[test]
    fn a_full_layout_is_refused_instead_of_dropping_a_row() {
        let rows = vec!["[\"agent\"]"; 16].join(", ");
        let text = format!("[ui.sidebar.agents]\nrows = [{rows}]\n");
        let error = plan(&text).unwrap_err();
        assert!(error.to_string().contains("16 rows"), "{error}");
    }

    #[test]
    fn install_backs_up_the_previous_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "onboarding = false\n").unwrap();
        install(&path, false).unwrap();
        let backups: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .contains("bak-organizations")
            })
            .collect();
        assert_eq!(backups.len(), 1);
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .contains("$org_task")
        );
        assert!(install(&path, false).unwrap().contains("already shows"));
    }
}

//! The optional dock: a split pane that runs the same TUI next to one
//! coordinator's agents. It reads the state file only. The toggle finds its
//! pane by a stored id plus an identity token the dock reports once, without
//! a TTL, so no heartbeat process is needed.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::herdr::{Herdr, Pane, PaneMetadata};
use crate::paths::Ctx;
use crate::project;
use crate::tui_config::{self, Dock};

pub const ACTION_ID: &str = "organization-sidebar";
const PROJECT_ENV: &str = "HERDR_ORGANIZATIONS_PROJECT";
const WORKSPACE_ENV: &str = "HERDR_ORGANIZATIONS_WORKSPACE";
const TOKEN_ID: &str = "org_dock";
const TOKEN_PROJECT: &str = "org_project";
const TOKEN_WORKSPACE: &str = "org_workspace";

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(default)]
struct Record {
    slug: String,
    workspace: String,
    socket: String,
    pane_id: String,
}

fn record_path(ctx: &Ctx, workspace: &str) -> PathBuf {
    let safe: String = workspace
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
        .collect();
    ctx.config_dir.join(format!(".dock-{safe}.json"))
}

fn save(ctx: &Ctx, record: &Record) -> Result<()> {
    std::fs::create_dir_all(&ctx.config_dir)?;
    project::write_json(&record_path(ctx, &record.workspace), record)
}

fn is_dock(pane: &Pane, slug: &str, workspace: &str) -> bool {
    let token = |name: &str| pane.tokens.get(name).and_then(serde_json::Value::as_str);
    pane.workspace_id == workspace
        && token(TOKEN_ID) == Some("v1")
        && token(TOKEN_PROJECT) == Some(slug)
        && token(TOKEN_WORKSPACE) == Some(workspace)
}

#[derive(Debug, PartialEq, Eq)]
pub enum Toggled {
    Opened,
    Closed,
}

/// Closes this workspace's dock when one is open, else opens it on the side
/// Settings chose. With the dock set to off it explains instead.
pub fn toggle(ctx: &Ctx, slug: &str, workspace: &str, source_pane: &str) -> Result<Toggled> {
    project::validate_slug(slug)?;
    if workspace.is_empty() {
        bail!("Herdr did not provide a workspace for the dock");
    }
    let socket = ctx
        .env
        .var("HERDR_SOCKET_PATH")
        .context("HERDR_SOCKET_PATH is not set for this Herdr action")?
        .to_string();
    let herdr = Herdr::new(ctx.env.herdr_bin(), &socket, ctx.runner);
    let lock = std::fs::File::create(ctx.config_dir.join(".dock.lock")).or_else(|_| {
        std::fs::create_dir_all(&ctx.config_dir)?;
        std::fs::File::create(ctx.config_dir.join(".dock.lock"))
    })?;
    lock.lock()?;

    let stored: Option<Record> = project::read_json(&record_path(ctx, workspace));
    let stored = stored.filter(|r| r.slug == slug && r.socket == socket && !r.pane_id.is_empty());
    let open: Vec<Pane> = match stored {
        Some(record) => match herdr.pane_get(&record.pane_id) {
            Ok(pane) if is_dock(&pane, slug, workspace) => vec![pane],
            _ => find(&herdr, slug, workspace)?,
        },
        None => find(&herdr, slug, workspace)?,
    };
    let mut record = Record {
        slug: slug.into(),
        workspace: workspace.into(),
        socket: socket.clone(),
        pane_id: String::new(),
    };
    if !open.is_empty() {
        for pane in open {
            match herdr.pane_close(&pane.pane_id) {
                Ok(()) => {}
                Err(error) if error.code == "pane_not_found" => {}
                Err(error) => return Err(anyhow::anyhow!("{error}")),
            }
        }
        save(ctx, &record)?;
        return Ok(Toggled::Closed);
    }

    let view = tui_config::load(&ctx.config_dir)?.view;
    if view.dock == Dock::Off {
        bail!("the dock is off; turn it on in Organizations settings (s) first");
    }
    if source_pane.is_empty() {
        bail!("Herdr did not provide a source pane for the dock");
    }
    let width = f64::from(view.dock_width) / 100.0;
    let ratio = if view.dock == Dock::Right {
        1.0 - width
    } else {
        width
    };
    let cwd = ctx.root.join(slug);
    let created = herdr
        .pane_split(source_pane, "right", ratio, &cwd.to_string_lossy())
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    record.pane_id = created.pane_id.clone();
    save(ctx, &record)?;
    let undo = |error: anyhow::Error| {
        let _ = herdr.pane_close(&created.pane_id);
        let _ = save(
            ctx,
            &Record {
                pane_id: String::new(),
                ..record.clone()
            },
        );
        error
    };
    if view.dock == Dock::Left {
        herdr
            .pane_swap(&created.pane_id, source_pane)
            .map_err(|e| undo(anyhow::anyhow!("{e}")))?;
    }
    herdr
        .pane_run(
            &created.pane_id,
            &launch_argv(ctx, &created.pane_id, slug, workspace, &socket)?,
        )
        .map_err(|e| undo(anyhow::anyhow!("{e}")))?;
    Ok(Toggled::Opened)
}

fn find(herdr: &Herdr, slug: &str, workspace: &str) -> Result<Vec<Pane>> {
    Ok(herdr
        .pane_list_workspace(workspace)
        .map_err(|e| anyhow::anyhow!("{e}"))?
        .into_iter()
        .filter(|p| is_dock(p, slug, workspace))
        .collect())
}

fn launch_argv(
    ctx: &Ctx,
    pane: &str,
    slug: &str,
    workspace: &str,
    socket: &str,
) -> Result<Vec<String>> {
    let binary =
        std::env::current_exe().context("could not locate the herdr-organizations binary")?;
    // `exec` replaces the split's shell, so the pane closes with the dock and
    // its identity tokens never outlive it on a live shell the toggle would
    // then close.
    let mut command = vec![
        "exec".to_string(),
        "env".to_string(),
        format!("HERDR_PANE_ID={pane}"),
        format!("HERDR_WORKSPACE_ID={workspace}"),
        format!("{PROJECT_ENV}={slug}"),
        format!("{WORKSPACE_ENV}={workspace}"),
        format!("HERDR_SOCKET_PATH={socket}"),
    ];
    for key in ["HERDR_BIN_PATH", "HERDR_PLUGIN_STATE_DIR"] {
        if let Some(value) = ctx.env.var(key) {
            command.push(format!("{key}={value}"));
        }
    }
    command.extend([
        binary.to_string_lossy().into_owned(),
        "--root".into(),
        ctx.root.to_string_lossy().into_owned(),
        "pane".into(),
        "dock".into(),
    ]);
    Ok(command)
}

/// The dock pane's own process: which project and workspace it shows.
pub fn context(ctx: &Ctx) -> Result<(String, String)> {
    let slug = ctx
        .env
        .var(PROJECT_ENV)
        .context("the dock was not given a project")?
        .to_string();
    project::validate_slug(&slug)?;
    let workspace = ctx
        .env
        .var(WORKSPACE_ENV)
        .context("the dock was not given a workspace")?
        .to_string();
    Ok((slug, workspace))
}

/// Reported once after the first frame. No TTL: the token lives as long as
/// the pane, which ends with the dock process (it was started with `exec`).
pub fn report_identity(ctx: &Ctx) {
    let (Ok((slug, workspace)), Some(pane), Some(socket)) = (
        context(ctx),
        ctx.env.var("HERDR_PANE_ID"),
        ctx.env.var("HERDR_SOCKET_PATH"),
    ) else {
        return;
    };
    let herdr = Herdr::new(ctx.env.herdr_bin(), socket, ctx.runner);
    let _ = herdr.pane_report_metadata_rpc(
        pane,
        &PaneMetadata {
            tokens: vec![
                (TOKEN_ID.into(), Some("v1".into())),
                (TOKEN_PROJECT.into(), Some(slug)),
                (TOKEN_WORKSPACE.into(), Some(workspace)),
            ],
            ttl: None,
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::Env;
    use crate::runner::fake::ok;
    use crate::scenarios::World;

    fn env(world: &World) -> Env {
        let socket = world.home.path().join("a.sock");
        Env::for_test(
            world.home.path(),
            &[("HERDR_SOCKET_PATH", socket.to_str().unwrap())],
        )
    }

    #[test]
    fn a_dock_set_to_off_explains_instead_of_splitting() {
        let world = World::new();
        world
            .runner
            .on("pane list", ok(r#"{"result":{"panes":[]}}"#));
        let env = env(&world);
        let ctx = Ctx {
            env: &env,
            ..world.ctx()
        };
        let error = toggle(&ctx, "demo", "w1", "w1:p1").unwrap_err();
        assert!(error.to_string().contains("dock is off"), "{error}");
        assert_eq!(world.runner.count("pane split"), 0);
    }

    #[test]
    fn toggle_opens_then_closes_the_pane_it_recognises() {
        let world = World::new();
        let env = env(&world);
        let ctx = Ctx {
            env: &env,
            ..world.ctx()
        };
        let mut config = tui_config::Config::default();
        config.view.dock = Dock::Right;
        tui_config::save(&ctx.config_dir, &config).unwrap();
        world
            .runner
            .on("pane list", ok(r#"{"result":{"panes":[]}}"#));
        world.runner.on(
            "pane split",
            ok(r#"{"result":{"pane":{"pane_id":"w1:p9","tab_id":"w1:t1","workspace_id":"w1"}}}"#),
        );
        world.runner.on("pane run", ok(""));
        assert_eq!(
            toggle(&ctx, "demo", "w1", "w1:p1").unwrap(),
            Toggled::Opened
        );
        let split = world
            .runner
            .calls
            .borrow()
            .iter()
            .find(|c| c.display().contains("pane split"))
            .unwrap()
            .display();
        assert!(split.contains("--ratio 0.7"), "{split}");
        let run = world
            .runner
            .calls
            .borrow()
            .iter()
            .find(|c| c.display().contains("pane run"))
            .unwrap()
            .display();
        assert!(run.contains("pane run w1:p9 exec env "), "{run}");

        world.runner.on(
            "pane get",
            ok(r#"{"result":{"pane":{"pane_id":"w1:p9","tab_id":"w1:t1","workspace_id":"w1","tokens":{"org_dock":"v1","org_project":"demo","org_workspace":"w1"}}}}"#),
        );
        world.runner.on("pane close", ok(r#"{"result":{}}"#));
        assert_eq!(
            toggle(&ctx, "demo", "w1", "w1:p1").unwrap(),
            Toggled::Closed
        );
        assert_eq!(world.runner.count("pane close w1:p9"), 1);
    }

    #[test]
    fn a_reused_pane_id_without_the_token_is_never_closed() {
        let world = World::new();
        let env = env(&world);
        let ctx = Ctx {
            env: &env,
            ..world.ctx()
        };
        save(
            &ctx,
            &Record {
                slug: "demo".into(),
                workspace: "w1".into(),
                socket: env.var("HERDR_SOCKET_PATH").unwrap().into(),
                pane_id: "w1:p9".into(),
            },
        )
        .unwrap();
        world.runner.on(
            "pane get",
            ok(r#"{"result":{"pane":{"pane_id":"w1:p9","tab_id":"w1:t1","workspace_id":"w1"}}}"#),
        );
        world
            .runner
            .on("pane list", ok(r#"{"result":{"panes":[]}}"#));
        let _ = toggle(&ctx, "demo", "w1", "w1:p1");
        assert_eq!(world.runner.count("pane close"), 0);
    }
}

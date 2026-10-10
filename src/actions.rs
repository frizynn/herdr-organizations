//! What herdr's action menu runs. Every interactive screen is the one
//! Organizations popup (`ui`); an action that already knows something (the
//! workspace to adopt, the command to pick a project for) hands it over in a
//! small file the popup reads once. The Projects popup (`projects`: tasks,
//! profiles and safety per project) stays available as its own pane.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};

use crate::adopt;
use crate::coordinator::{self, OpenOptions};
use crate::herdr::{CALL_TIMEOUT, Herdr};
use crate::paths::{Ctx, SessionFlags};
use crate::project::Status;
use crate::ui::{self, Handoff};
use crate::{dock, doctor, lifecycle, overview};

const PLUGIN_ID: &str = "herdr-projects";
pub const POPUP: &str = "ui";
/// The Projects popup's pane.
pub const PROJECTS: &str = "projects";

/// What the `projects` action hands to the Projects popup.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default, PartialEq)]
#[serde(default)]
pub struct ProjectsHandoff {
    pub slug: String,
    /// The workspace the popup was opened from (new tabs go there).
    pub workspace_id: String,
}

fn projects_handoff_file(ctx: &Ctx) -> Result<PathBuf> {
    let dir = ctx.env.var("HERDR_PLUGIN_STATE_DIR").context(
        "HERDR_PLUGIN_STATE_DIR is not set: this command is meant to be run by herdr as a plugin action or pane",
    )?;
    Ok(PathBuf::from(dir).join("handoff.json"))
}

fn read_projects_handoff(ctx: &Ctx) -> ProjectsHandoff {
    projects_handoff_file(ctx)
        .ok()
        .and_then(|file| crate::project::read_json(&file))
        .unwrap_or_default()
}

/// The originating pane and workspace, from `HERDR_PLUGIN_CONTEXT_JSON`.
#[derive(Debug, Default, serde::Deserialize)]
#[serde(default)]
pub(crate) struct ActionContext {
    workspace_id: String,
    pub(crate) workspace_label: String,
    workspace_cwd: String,
    focused_pane_id: String,
}

pub(crate) fn action_context(ctx: &Ctx) -> ActionContext {
    ctx.env
        .var("HERDR_PLUGIN_CONTEXT_JSON")
        .and_then(|json| serde_json::from_str(json).ok())
        .unwrap_or_default()
}

fn socket(ctx: &Ctx) -> Result<String> {
    ctx.env
        .var("HERDR_SOCKET_PATH")
        .map(str::to_string)
        .context("HERDR_SOCKET_PATH is not set: this command is meant to be run by herdr")
}

pub fn open_popup(ctx: &Ctx) -> Result<()> {
    open_entrypoint(ctx, POPUP)
}

fn open_entrypoint(ctx: &Ctx, entrypoint: &str) -> Result<()> {
    let herdr = Herdr::new(ctx.env.herdr_bin(), socket(ctx)?, ctx.runner);
    herdr
        .call(
            &[
                "plugin",
                "pane",
                "open",
                "--plugin",
                PLUGIN_ID,
                "--entrypoint",
                entrypoint,
            ],
            CALL_TIMEOUT,
        )
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(())
}

fn open_with(ctx: &Ctx, handoff: Handoff) -> Result<()> {
    ui::write_handoff(ctx, &handoff)?;
    open_popup(ctx)
}

pub(crate) fn current_workspace(ctx: &Ctx, context: &ActionContext) -> String {
    ctx.env
        .var("HERDR_WORKSPACE_ID")
        .unwrap_or(&context.workspace_id)
        .to_string()
}

fn current_pane(ctx: &Ctx, context: &ActionContext) -> String {
    ctx.env
        .var("HERDR_PANE_ID")
        .unwrap_or(&context.focused_pane_id)
        .to_string()
}

/// The project of the workspace the action was invoked from, if any.
fn current_slug(ctx: &Ctx) -> Option<String> {
    let context = action_context(ctx);
    overview::project_for_workspace(
        ctx,
        &current_workspace(ctx, &context),
        ctx.env.var("HERDR_SOCKET_PATH").unwrap_or(""),
    )
}

pub fn run_action(ctx: &Ctx, id: &str) -> Result<()> {
    let context = action_context(ctx);
    match id {
        // `open-popup` is the id the popup key binds (`configure`).
        "organizations" | "open-popup" => open_popup(ctx),
        PROJECTS => {
            let file = projects_handoff_file(ctx)?;
            if let Some(dir) = file.parent() {
                std::fs::create_dir_all(dir)?;
            }
            crate::project::write_json(
                &file,
                &ProjectsHandoff {
                    slug: current_slug(ctx).unwrap_or_default(),
                    workspace_id: current_workspace(ctx, &context),
                },
            )?;
            open_entrypoint(ctx, PROJECTS)
        }
        "new" => open_with(
            ctx,
            Handoff {
                screen: "new".into(),
                ..Handoff::default()
            },
        ),
        "overview" => open_with(
            ctx,
            Handoff {
                screen: "tree".into(),
                slug: current_slug(ctx).unwrap_or_default(),
                ..Handoff::default()
            },
        ),
        dock::ACTION_ID => {
            let slug = current_slug(ctx).context(
                "the dock needs a Herdr Organizations project workspace; use the launcher to browse all projects",
            )?;
            let workspace = current_workspace(ctx, &context);
            let pane = current_pane(ctx, &context);
            dock::toggle(ctx, &slug, &workspace, &pane).map(|_| ())
        }
        "open" | "pause" | "resume" => match current_slug(ctx) {
            Some(slug) => run_on_slug(ctx, id, &slug),
            None => open_with(
                ctx,
                Handoff {
                    screen: "pick".into(),
                    command: id.to_string(),
                    ..Handoff::default()
                },
            ),
        },
        "focus" => match current_slug(ctx) {
            Some(slug) => overview::focus(ctx, Some(&slug)),
            None => bail!(
                "this workspace does not belong to a project; run `focus <slug>` from a terminal"
            ),
        },
        "unfocus" => overview::unfocus(ctx, &SessionFlags::default()),
        "adopt-workspace" => {
            // The originating pane is captured here, before any popup opens.
            let pane = current_pane(ctx, &context);
            if pane.is_empty() {
                bail!("herdr did not say which pane this action was invoked from");
            }
            let herdr = Herdr::new(ctx.env.herdr_bin(), socket(ctx)?, ctx.runner);
            adopt::adoptable_agent(ctx, &herdr, &socket(ctx)?, &pane)?;
            open_with(
                ctx,
                Handoff {
                    screen: "adopt".into(),
                    pane_id: pane,
                    workspace_label: context.workspace_label,
                    workspace_cwd: context.workspace_cwd,
                    ..Handoff::default()
                },
            )
        }
        "configure" => {
            let options = crate::setup::ConfigureOptions {
                clients: vec![],
                claude_home: None,
                codex_home: None,
                dry_run: false,
                hooks: true,
                sidebar: true,
                key: None,
                herdr_config: None,
                skill: crate::setup::skill_source(),
            };
            let herdr = Herdr::new(ctx.env.herdr_bin(), socket(ctx)?, ctx.runner);
            match crate::setup::configure(ctx, &options) {
                Ok(notes) => {
                    for note in &notes {
                        println!("{note}");
                    }
                    crate::setup::apply_live(ctx);
                    let _ = herdr.notification_show(
                        "Herdr Organizations configured",
                        "Sidebar rows, popup key, progress hooks and the autoproject skill are set. Run `reload config` if the rows are not visible yet.",
                    );
                }
                Err(error) => {
                    let _ = herdr.notification_show(
                        "Herdr Organizations: configure failed",
                        &format!("{error:#}"),
                    );
                    return Err(error);
                }
            }
            Ok(())
        }
        "doctor" => {
            let healthy = doctor::run(ctx, &SessionFlags::default(), false)?;
            let herdr = Herdr::new(ctx.env.herdr_bin(), socket(ctx)?, ctx.runner);
            let body = if healthy {
                "All required checks passed. Details: herdr plugin log --plugin herdr-projects"
            } else {
                "Some checks FAILED. Details: herdr plugin log --plugin herdr-projects"
            };
            let _ = herdr.notification_show("Herdr Organizations doctor", body);
            Ok(())
        }
        other => bail!("unknown action `{other}`"),
    }
}

pub fn run_on_slug(ctx: &Ctx, command: &str, slug: &str) -> Result<()> {
    match command {
        "open" => coordinator::open(
            ctx,
            slug,
            &OpenOptions {
                session: SessionFlags {
                    session: None,
                    socket: Some(PathBuf::from(socket(ctx)?)),
                },
                rebind: false,
                profile: None,
                new: false,
                here: false,
            },
        ),
        "pause" => lifecycle::set_status(ctx, slug, Status::Paused),
        "resume" => lifecycle::set_status(ctx, slug, Status::Active),
        other => bail!("`{other}` cannot be run from the picker"),
    }
}

/// A plugin pane's process: the popup, or the dock in its split.
pub fn run_pane(ctx: &Ctx, id: &str) -> Result<()> {
    match id {
        POPUP => ui::run_popup(ctx),
        PROJECTS => {
            let handoff = read_projects_handoff(ctx);
            crate::popup::run(
                ctx,
                Some(handoff.slug).filter(|s| !s.is_empty()),
                handoff.workspace_id,
            )
        }
        "dock" => {
            let (slug, workspace) = dock::context(ctx)?;
            ui::run_dock(ctx, slug, workspace)
        }
        other => bail!("unknown pane `{other}`"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::Env;
    use crate::runner::fake::ok;
    use crate::scenarios::World;

    fn manifest() -> toml::Value {
        toml::from_str(include_str!("../herdr-plugin.toml")).unwrap()
    }

    fn command(entry: &toml::Value) -> Vec<&str> {
        entry["command"]
            .as_array()
            .unwrap()
            .iter()
            .map(|argument| argument.as_str().unwrap())
            .collect()
    }

    #[test]
    fn manifest_declares_one_popup_for_every_interactive_screen() {
        let manifest = manifest();
        assert_eq!(manifest["id"].as_str(), Some(PLUGIN_ID));
        assert_eq!(manifest["name"].as_str(), Some("Herdr Organizations"));
        let panes = manifest["panes"].as_array().unwrap();
        assert_eq!(
            panes.len(),
            2,
            "launcher, new, tree, board, detail and settings share one popup; Projects has its own"
        );
        assert_eq!(panes[0]["id"].as_str(), Some(POPUP));
        assert_eq!(panes[0]["placement"].as_str(), Some("popup"));
        assert_eq!(
            command(&panes[0]),
            ["target/release/herdr-organizations", "pane", POPUP]
        );
        assert_eq!(
            command(&panes[1]),
            ["target/release/herdr-organizations", "pane", PROJECTS]
        );
        let actions = manifest["actions"].as_array().unwrap();
        // The popup key `configure` binds runs `open-popup`.
        let key = actions
            .iter()
            .find(|entry| entry["id"].as_str() == Some("open-popup"))
            .unwrap();
        assert_eq!(
            command(key),
            ["target/release/herdr-organizations", "action", "open-popup"]
        );
        let launcher = actions
            .iter()
            .find(|entry| entry["id"].as_str() == Some("organizations"))
            .unwrap();
        assert_eq!(
            command(launcher),
            [
                "target/release/herdr-organizations",
                "action",
                "organizations"
            ]
        );
        let dock = actions
            .iter()
            .find(|entry| entry["id"].as_str() == Some(dock::ACTION_ID))
            .unwrap();
        assert_eq!(
            command(dock),
            [
                "target/release/herdr-organizations",
                "action",
                dock::ACTION_ID
            ]
        );
        // No event hook spawns a process on every tab focus any more.
        assert!(manifest.get("events").is_none());
    }

    fn plugin_env(world: &World, extra: &[(&str, &str)]) -> Env {
        let state = world.home.path().join("state");
        let socket = world.home.path().join("a.sock");
        let mut vars = vec![
            (
                "HERDR_PLUGIN_STATE_DIR",
                state.to_str().unwrap().to_string(),
            ),
            ("HERDR_SOCKET_PATH", socket.to_str().unwrap().to_string()),
        ];
        vars.extend(extra.iter().map(|(k, v)| (*k, v.to_string())));
        let refs: Vec<(&str, &str)> = vars.iter().map(|(k, v)| (*k, v.as_str())).collect();
        Env::for_test(world.home.path(), &refs)
    }

    fn opened_popup(world: &World) -> bool {
        world.runner.calls.borrow().iter().any(|call| {
            call.args
                == [
                    "plugin",
                    "pane",
                    "open",
                    "--plugin",
                    "herdr-projects",
                    "--entrypoint",
                    POPUP,
                ]
        })
    }

    #[test]
    fn an_action_without_a_project_opens_the_launcher_as_a_picker() {
        let world = World::new();
        world.project("demo", "a.sock");
        world
            .runner
            .on("plugin pane open", ok(r#"{"result":{"type":"ok"}}"#));
        let env = plugin_env(&world, &[("HERDR_WORKSPACE_ID", "w42")]);
        let ctx = Ctx {
            env: &env,
            ..world.ctx()
        };
        run_action(&ctx, "pause").unwrap();
        assert!(opened_popup(&world));
        let handoff = ui::take_handoff(&ctx);
        assert_eq!(
            (handoff.screen.as_str(), handoff.command.as_str()),
            ("pick", "pause")
        );
    }

    #[test]
    fn launcher_and_new_open_the_same_popup() {
        let world = World::new();
        world
            .runner
            .on("plugin pane open", ok(r#"{"result":{"type":"ok"}}"#));
        let env = plugin_env(&world, &[]);
        let ctx = Ctx {
            env: &env,
            ..world.ctx()
        };
        run_action(&ctx, "organizations").unwrap();
        assert!(opened_popup(&world));
        assert_eq!(ui::take_handoff(&ctx), Handoff::default());
        run_action(&ctx, "new").unwrap();
        assert_eq!(ui::take_handoff(&ctx).screen, "new");
    }

    #[test]
    fn dock_action_refuses_outside_a_project_workspace() {
        let world = World::new();
        let env = plugin_env(&world, &[]);
        let ctx = Ctx {
            env: &env,
            ..world.ctx()
        };
        let error = run_action(&ctx, dock::ACTION_ID).unwrap_err();
        assert!(error.to_string().contains("project workspace"));
        assert_eq!(world.runner.count("pane split"), 0);
    }

    #[test]
    fn popup_handoffs_are_scoped_to_action_correlation_ids() {
        let world = World::new();
        world
            .runner
            .on("plugin pane open", ok(r#"{"result":{"type":"ok"}}"#));
        let first = plugin_env(
            &world,
            &[(
                "HERDR_PLUGIN_CONTEXT_JSON",
                r#"{"workspace_id":"w41","correlation_id":"invocation-one"}"#,
            )],
        );
        let second = plugin_env(
            &world,
            &[(
                "HERDR_PLUGIN_CONTEXT_JSON",
                r#"{"workspace_id":"w42","correlation_id":"invocation-two"}"#,
            )],
        );
        let first_ctx = Ctx {
            env: &first,
            ..world.ctx()
        };
        let second_ctx = Ctx {
            env: &second,
            ..world.ctx()
        };
        run_action(&first_ctx, "pause").unwrap();
        run_action(&second_ctx, "resume").unwrap();
        assert_eq!(ui::take_handoff(&first_ctx).command, "pause");
        assert_eq!(ui::take_handoff(&second_ctx).command, "resume");
    }

    #[test]
    fn an_action_inside_a_project_workspace_acts_on_that_project() {
        let world = World::new();
        let project = world.project("demo", "a.sock");
        let env = plugin_env(&world, &[("HERDR_WORKSPACE_ID", "w1")]);
        let ctx = Ctx {
            env: &env,
            ..world.ctx()
        };
        run_action(&ctx, "pause").unwrap();
        assert_eq!(project.status(), Status::Paused);
        assert_eq!(world.runner.count("plugin pane open"), 0);
        run_action(&ctx, "resume").unwrap();
        assert_eq!(project.status(), Status::Active);
    }

    #[test]
    fn adopt_workspace_captures_the_pane_in_the_action_and_refuses_without_an_agent() {
        let world = World::new();
        world
            .runner
            .on("plugin pane open", ok(r#"{"result":{"type":"ok"}}"#));
        let context = r#"{"workspace_id":"w5","workspace_label":"My Repo","workspace_cwd":"/work","focused_pane_id":"w5:p1","correlation_id":"adopt-invocation"}"#;
        let env = plugin_env(&world, &[("HERDR_PLUGIN_CONTEXT_JSON", context)]);
        let ctx = Ctx {
            env: &env,
            ..world.ctx()
        };
        // No agent in the pane: refused before any popup opens.
        assert!(run_action(&ctx, "adopt-workspace").is_err());
        assert_eq!(world.runner.count("plugin pane open"), 0);

        *world.agents.borrow_mut() = format!(
            "[{}]",
            crate::scenarios::agent_json("w5", "w5:t1", "w5:p1", "/work", "", "idle")
        );
        run_action(&ctx, "adopt-workspace").unwrap();
        let handoff = ui::take_handoff(&ctx);
        assert_eq!(
            (
                handoff.screen.as_str(),
                handoff.pane_id.as_str(),
                handoff.workspace_label.as_str(),
                handoff.workspace_cwd.as_str()
            ),
            ("adopt", "w5:p1", "My Repo", "/work")
        );
    }

    #[test]
    fn the_projects_popup_opens_scoped_to_the_focused_workspaces_project_and_on_all_projects_elsewhere()
     {
        let world = World::new();
        let project = world.project("demo", "a.sock");
        world
            .runner
            .on("plugin pane open", ok(r#"{"result":{"type":"ok"}}"#));
        // The coordinator was reopened in w9; the record still says w1.
        *world.panes.borrow_mut() = format!(
            "[{}]",
            crate::scenarios::pane_json(
                "w9",
                "w9:t1",
                "w9:p1",
                &project.canonical_dir().to_string_lossy()
            )
        );
        let inside = plugin_env(
            &world,
            &[("HERDR_WORKSPACE_ID", "w9"), ("HERDR_PANE_ID", "w9:p1")],
        );
        let ctx = Ctx {
            env: &inside,
            ..world.ctx()
        };
        run_action(&ctx, PROJECTS).unwrap();
        let handoff = read_projects_handoff(&ctx);
        assert_eq!(
            (handoff.slug.as_str(), handoff.workspace_id.as_str()),
            ("demo", "w9")
        );

        let outside = plugin_env(&world, &[("HERDR_WORKSPACE_ID", "w3")]);
        let ctx = Ctx {
            env: &outside,
            ..world.ctx()
        };
        run_action(&ctx, PROJECTS).unwrap();
        assert_eq!(read_projects_handoff(&ctx).slug, "");
    }

    #[test]
    fn the_popup_key_opens_the_organizations_popup() {
        let world = World::new();
        world
            .runner
            .on("plugin pane open", ok(r#"{"result":{"type":"ok"}}"#));
        let env = plugin_env(&world, &[]);
        let ctx = Ctx {
            env: &env,
            ..world.ctx()
        };
        run_action(&ctx, "open-popup").unwrap();
        assert!(opened_popup(&world));
    }
}

//! What herdr's action menu and popup panes run. An action that needs
//! to ask the user something opens its popup with `herdr plugin pane open`,
//! passing what it already knows through a small file in the plugin state dir.

use std::io::{BufRead, Write as _};
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::adopt::{self, AdoptWorkspace};
use crate::coordinator::{self, OpenOptions};
use crate::herdr::{CALL_TIMEOUT, Herdr};
use crate::paths::{Ctx, SessionFlags};
use crate::project::{self, Status};
use crate::{doctor, lifecycle, organization_sidebar, organizations_ui, overview};

const PLUGIN_ID: &str = "herdr-projects";

/// What an action hands to the popup it opens.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(default)]
pub struct Handoff {
    /// The subcommand the `pick` pane should run: `open`, `pause` or `resume`.
    pub command: String,
    pub slug: String,
    /// Captured by the action, before any popup opens.
    pub pane_id: String,
    pub workspace_label: String,
    pub workspace_cwd: String,
    pub socket: String,
}

/// The originating pane and workspace, from the action's own environment or
/// from `HERDR_PLUGIN_CONTEXT_JSON`.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ActionContext {
    workspace_id: String,
    tab_id: String,
    workspace_label: String,
    workspace_cwd: String,
    focused_pane_id: String,
    correlation_id: String,
}

fn action_context(ctx: &Ctx) -> ActionContext {
    ctx.env
        .var("HERDR_PLUGIN_CONTEXT_JSON")
        .and_then(|json| serde_json::from_str(json).ok())
        .unwrap_or_default()
}

fn state_file(ctx: &Ctx) -> Result<PathBuf> {
    let dir = ctx.env.var("HERDR_PLUGIN_STATE_DIR").context("HERDR_PLUGIN_STATE_DIR is not set: this command is meant to be run by herdr as a plugin action or pane")?;
    let context = action_context(ctx);
    let file_name = if !context.correlation_id.is_empty()
        && context.correlation_id.len() <= 128
        && context
            .correlation_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        format!("handoff-{}.json", context.correlation_id)
    } else {
        "handoff.json".into()
    };
    Ok(PathBuf::from(dir).join(file_name))
}

fn socket(ctx: &Ctx) -> Result<String> {
    ctx.env
        .var("HERDR_SOCKET_PATH")
        .map(str::to_string)
        .context("HERDR_SOCKET_PATH is not set: this command is meant to be run by herdr")
}

fn open_pane(ctx: &Ctx, entrypoint: &str, handoff: Option<&Handoff>) -> Result<()> {
    if let Some(handoff) = handoff {
        let file = state_file(ctx)?;
        if let Some(dir) = file.parent() {
            std::fs::create_dir_all(dir)?;
        }
        project::write_json(&file, handoff)?;
    }
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

fn read_handoff(ctx: &Ctx) -> Handoff {
    state_file(ctx)
        .ok()
        .and_then(|file| project::read_json(&file))
        .unwrap_or_default()
}

/// The project of the workspace the action was invoked from, if any.
fn current_slug(ctx: &Ctx) -> Option<String> {
    let context = action_context(ctx);
    let workspace = ctx
        .env
        .var("HERDR_WORKSPACE_ID")
        .map(str::to_string)
        .unwrap_or(context.workspace_id);
    overview::project_for_workspace(
        ctx,
        &workspace,
        ctx.env.var("HERDR_SOCKET_PATH").unwrap_or(""),
    )
}

fn current_workspace(ctx: &Ctx, context: &ActionContext) -> String {
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

fn current_tab(ctx: &Ctx, context: &ActionContext) -> String {
    ctx.env
        .var("HERDR_TAB_ID")
        .unwrap_or(&context.tab_id)
        .to_string()
}

pub fn run_action(ctx: &Ctx, id: &str) -> Result<()> {
    let context = action_context(ctx);
    let base = Handoff {
        socket: socket(ctx).unwrap_or_default(),
        ..Handoff::default()
    };
    match id {
        "new" => open_pane(ctx, "new", None),
        "organizations" => open_pane(ctx, "organizations", None),
        organization_sidebar::ACTION_ID => {
            let slug = current_slug(ctx).context(
                "this action needs a Herdr Organizations project workspace; use `organizations` to browse all projects",
            )?;
            let workspace = current_workspace(ctx, &context);
            let tab = current_tab(ctx, &context);
            let pane = current_pane(ctx, &context);
            organization_sidebar::toggle(ctx, &slug, &workspace, &tab, &pane).map(|_| ())
        }
        organization_sidebar::AUTO_OPEN_ACTION_ID => {
            let workspace = current_workspace(ctx, &context);
            let tab = current_tab(ctx, &context);
            let pane = current_pane(ctx, &context);
            organization_sidebar::ensure_auto_open(
                ctx,
                current_slug(ctx).as_deref(),
                &workspace,
                &tab,
                &pane,
            )
            .map(|_| ())
        }
        "overview" => open_pane(
            ctx,
            "overview",
            Some(&Handoff {
                slug: current_slug(ctx).unwrap_or_default(),
                ..base
            }),
        ),
        "open" | "pause" | "resume" => match current_slug(ctx) {
            Some(slug) => run_on_slug(ctx, id, &slug),
            None => open_pane(
                ctx,
                "pick",
                Some(&Handoff {
                    command: id.to_string(),
                    ..base
                }),
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
            let pane = ctx
                .env
                .var("HERDR_PANE_ID")
                .map(str::to_string)
                .unwrap_or(context.focused_pane_id);
            if pane.is_empty() {
                bail!("herdr did not say which pane this action was invoked from");
            }
            let herdr = Herdr::new(ctx.env.herdr_bin(), socket(ctx)?, ctx.runner);
            adopt::adoptable_agent(ctx, &herdr, &socket(ctx)?, &pane)?;
            open_pane(
                ctx,
                "adopt",
                Some(&Handoff {
                    pane_id: pane,
                    workspace_label: context.workspace_label,
                    workspace_cwd: context.workspace_cwd,
                    ..base
                }),
            )
        }
        "doctor" => {
            let healthy = doctor::run(ctx, &SessionFlags::default())?;
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

fn run_on_slug(ctx: &Ctx, command: &str, slug: &str) -> Result<()> {
    match command {
        "open" => coordinator::open(
            ctx,
            slug,
            &OpenOptions {
                session: SessionFlags {
                    session: None,
                    socket: Some(PathBuf::from(socket(ctx)?)),
                },
                reprime: false,
                rebind: false,
            },
        ),
        "pause" => lifecycle::set_status(ctx, slug, Status::Paused),
        "resume" => lifecycle::set_status(ctx, slug, Status::Active),
        other => bail!("`{other}` cannot be run from the picker"),
    }
}

fn ask(question: &str, default: &str) -> Result<String> {
    if default.is_empty() {
        print!("{question}: ");
    } else {
        print!("{question} [{default}]: ");
    }
    std::io::stdout().flush()?;
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line)?;
    let answer = line.trim();
    Ok(if answer.is_empty() {
        default.to_string()
    } else {
        answer.to_string()
    })
}

fn hold_open() {
    let _ = ask("\nPress Enter to close", "");
}

/// A static plugin pane's body. Modal panes show failures long enough to read;
/// the contextual sidebar handles and displays its own interactive errors.
pub fn run_pane(ctx: &Ctx, id: &str) -> Result<()> {
    if id == "organization-sidebar" {
        return match organization_sidebar::run(ctx) {
            Ok(()) => Ok(()),
            Err(error) => {
                println!("\nerror: {error:#}");
                hold_open();
                Err(error)
            }
        };
    }
    if id == "organizations" {
        return organizations_ui::run(ctx);
    }
    if id == "overview" {
        let handoff = read_handoff(ctx);
        return overview::run(
            ctx,
            Some(handoff.slug.as_str()).filter(|slug| !slug.is_empty()),
            true,
        );
    }
    let handoff = if id == "new" {
        Handoff::default()
    } else {
        read_handoff(ctx)
    };
    let result = match id {
        "new" => (|| {
            println!("New project\n");
            let name = ask("Name", "")?;
            if name.is_empty() {
                bail!("no name given");
            }
            let goal = ask("Goal (one line, optional)", "")?;
            let project = project::create(&ctx.root, &name, &goal, Vec::new())?;
            println!("created `{}` at {}", project.slug, project.dir().display());
            run_on_slug(ctx, "open", &project.slug)
        })(),
        "pick" => (|| {
            println!("Which project should `{}` act on?\n", handoff.command);
            let slug = overview::pick(ctx)?;
            run_on_slug(ctx, &handoff.command, &slug)
        })(),
        "adopt" => (|| {
            println!("Continue this workspace as a project\n");
            let name = ask("Project name", &handoff.workspace_label)?;
            if name.is_empty() {
                bail!("no name given");
            }
            adopt::adopt_workspace(
                ctx,
                &AdoptWorkspace {
                    name,
                    goal: String::new(),
                    pane: handoff.pane_id.clone(),
                    workspace_cwd: handoff.workspace_cwd.clone(),
                    session: SessionFlags {
                        session: None,
                        socket: Some(PathBuf::from(socket(ctx)?)),
                    },
                },
            )
        })(),
        other => bail!("unknown pane `{other}`"),
    };
    if let Err(error) = &result {
        println!("\nerror: {error:#}");
    }
    hold_open();
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::Env;
    use crate::runner::fake::ok;
    use crate::scenarios::World;

    #[test]
    fn plugin_manifest_registers_the_organizations_action_and_pane() {
        let manifest: toml::Value = toml::from_str(include_str!("../herdr-plugin.toml")).unwrap();
        assert_eq!(manifest["id"].as_str(), Some(PLUGIN_ID));
        assert_eq!(manifest["name"].as_str(), Some("Herdr Organizations"));
        let organization_action = manifest["actions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["id"].as_str() == Some("organizations"))
            .unwrap();
        assert_eq!(
            organization_action["title"].as_str(),
            Some("Herdr Organizations: organization tree")
        );
        assert_eq!(
            organization_action["command"]
                .as_array()
                .unwrap()
                .iter()
                .map(|argument| argument.as_str().unwrap())
                .collect::<Vec<_>>(),
            [
                "target/release/herdr-organizations",
                "action",
                "organizations"
            ]
        );
        let organization_pane = manifest["panes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["id"].as_str() == Some("organizations"))
            .unwrap();
        assert_eq!(
            organization_pane["title"].as_str(),
            Some("Herdr Organizations")
        );
        assert_eq!(
            organization_pane["command"]
                .as_array()
                .unwrap()
                .iter()
                .map(|argument| argument.as_str().unwrap())
                .collect::<Vec<_>>(),
            [
                "target/release/herdr-organizations",
                "pane",
                "organizations"
            ]
        );
    }

    #[test]
    fn plugin_manifest_keeps_the_global_picker_and_adds_a_contextual_sidebar_action() {
        let manifest: toml::Value = toml::from_str(include_str!("../herdr-plugin.toml")).unwrap();
        let actions = manifest["actions"].as_array().unwrap();
        let global = actions
            .iter()
            .find(|entry| entry["id"].as_str() == Some("organizations"))
            .unwrap();
        let sidebar = actions
            .iter()
            .find(|entry| entry["id"].as_str() == Some("organization-sidebar"))
            .unwrap();
        assert_eq!(
            global["title"].as_str(),
            Some("Herdr Organizations: organization tree")
        );
        assert_eq!(
            sidebar["command"]
                .as_array()
                .unwrap()
                .iter()
                .map(|argument| argument.as_str().unwrap())
                .collect::<Vec<_>>(),
            [
                "target/release/herdr-organizations",
                "action",
                "organization-sidebar"
            ]
        );
        assert!(manifest["events"].as_array().unwrap().iter().any(|event| {
            event["command"].as_array().is_some_and(|command| {
                command
                    .iter()
                    .any(|argument| argument.as_str() == Some("organization-sidebar-auto-open"))
            })
        }));
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

    #[test]
    fn an_action_without_a_project_opens_the_picker_with_the_requested_command() {
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
        let calls = world.runner.calls.borrow();
        let opened = calls
            .iter()
            .find(|c| c.display().contains("plugin pane open"))
            .unwrap();
        assert!(
            opened
                .display()
                .contains("--plugin herdr-projects --entrypoint pick")
        );
        drop(calls);
        assert_eq!(read_handoff(&ctx).command, "pause");
    }

    #[test]
    fn organizations_action_opens_without_reading_or_writing_a_handoff() {
        let world = World::new();
        world
            .runner
            .on("plugin pane open", ok(r#"{"result":{"type":"ok"}}"#));
        let env = plugin_env(&world, &[]);
        let state_dir = world.home.path().join("state");
        let ctx = Ctx {
            env: &env,
            ..world.ctx()
        };

        run_action(&ctx, "organizations").unwrap();

        assert!(!state_dir.exists());
        let call = world
            .runner
            .calls
            .borrow()
            .iter()
            .find(|call| {
                call.args
                    .starts_with(&["plugin".into(), "pane".into(), "open".into()])
            })
            .cloned()
            .unwrap();
        assert_eq!(
            call.args,
            [
                "plugin",
                "pane",
                "open",
                "--plugin",
                "herdr-projects",
                "--entrypoint",
                "organizations"
            ]
        );
    }

    #[test]
    fn contextual_sidebar_action_refuses_to_open_from_a_non_project_workspace() {
        let world = World::new();
        let env = plugin_env(&world, &[]);
        let ctx = Ctx {
            env: &env,
            ..world.ctx()
        };

        let error = run_action(&ctx, "organization-sidebar").unwrap_err();

        assert!(error.to_string().contains("project workspace"));
        assert_eq!(world.runner.count("pane split"), 0);
        assert_eq!(world.runner.count("plugin pane open"), 0);
    }

    #[test]
    fn new_action_opens_without_a_handoff_file() {
        let world = World::new();
        world
            .runner
            .on("plugin pane open", ok(r#"{"result":{"type":"ok"}}"#));
        let env = plugin_env(&world, &[]);
        let state_dir = world.home.path().join("state");
        let ctx = Ctx {
            env: &env,
            ..world.ctx()
        };

        run_action(&ctx, "new").unwrap();

        assert!(!state_dir.exists());
        assert!(
            world
                .runner
                .calls
                .borrow()
                .iter()
                .any(|call| { call.args.ends_with(&["--entrypoint".into(), "new".into()]) })
        );
    }

    #[test]
    fn popup_handoffs_are_scoped_to_action_correlation_ids() {
        let world = World::new();
        world
            .runner
            .on("plugin pane open", ok(r#"{"result":{"type":"ok"}}"#));
        let first_context = r#"{"workspace_id":"w41","correlation_id":"invocation-one"}"#;
        let second_context = r#"{"workspace_id":"w42","correlation_id":"invocation-two"}"#;
        let first_env = plugin_env(&world, &[("HERDR_PLUGIN_CONTEXT_JSON", first_context)]);
        let second_env = plugin_env(&world, &[("HERDR_PLUGIN_CONTEXT_JSON", second_context)]);
        let first_ctx = Ctx {
            env: &first_env,
            ..world.ctx()
        };
        let second_ctx = Ctx {
            env: &second_env,
            ..world.ctx()
        };

        run_action(&first_ctx, "pause").unwrap();
        run_action(&second_ctx, "resume").unwrap();

        assert_eq!(read_handoff(&first_ctx).command, "pause");
        assert_eq!(read_handoff(&second_ctx).command, "resume");
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
        let handoff = read_handoff(&ctx);
        assert_eq!(
            (
                handoff.pane_id.as_str(),
                handoff.workspace_label.as_str(),
                handoff.workspace_cwd.as_str()
            ),
            ("w5:p1", "My Repo", "/work")
        );
    }
}

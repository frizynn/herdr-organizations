//! Node templates: a node's role, profile name, rules and memory, saved to
//! start other nodes from. A global template lives in the config folder, a
//! project's own under the project; with a project, its own wins over a
//! global one of the same name. The launch setup is a profile name only:
//! per-node model, effort, permission and raw-argument flags are not kept,
//! so a setup worth reusing is a profile (`profile add`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::organizations::{self, Memory};
use crate::paths::Ctx;
use crate::project::{self, Project};
use crate::thread::{self, NodeRole};
use crate::threads::StartArgs;

const DIR: &str = "templates";
const SPEC: &str = "TEMPLATE.toml";
const RULES: &str = "RULES.md";

/// `TEMPLATE.toml`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Spec {
    pub role: NodeRole,
    pub can_spawn: bool,
    /// Empty: the new node's parent's, else the project's default for its role.
    pub profile: String,
    pub description: String,
    pub created: String,
    pub updated: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    Global,
    Project(String),
}

impl Scope {
    fn of(slug: &str) -> Scope {
        Scope::Project(slug.to_string())
    }

    fn word(&self) -> &'static str {
        match self {
            Scope::Global => "global",
            Scope::Project(_) => "project",
        }
    }

    fn slug(&self) -> Option<&str> {
        match self {
            Scope::Global => None,
            Scope::Project(slug) => Some(slug),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Template {
    pub name: String,
    pub scope: Scope,
    pub dir: PathBuf,
    pub spec: Spec,
}

fn scope_dir(ctx: &Ctx, scope: &Scope) -> Result<PathBuf> {
    Ok(match scope {
        Scope::Global => ctx.config_dir.join(DIR),
        Scope::Project(slug) => Project::load(&ctx.root, slug)?.dir().join(DIR),
    })
}

/// The templates `project` can use, by name: its own over global ones of the
/// same name. Without a project, the global ones. A folder that is not a
/// valid template is skipped with a warning.
pub fn list(ctx: &Ctx, project: Option<&str>) -> Result<Vec<Template>> {
    let mut found = BTreeMap::new();
    for scope in std::iter::once(Scope::Global).chain(project.map(Scope::of)) {
        let dir = scope_dir(ctx, &scope)?;
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(error).with_context(|| format!("could not list {}", dir.display()));
            }
        };
        for entry in entries {
            let name = entry?.file_name().to_string_lossy().into_owned();
            if project::validate_slug(&name).is_err() {
                continue;
            }
            match load(&dir.join(&name), &name, scope.clone()) {
                Ok(template) => {
                    found.insert(name, template);
                }
                Err(error) => eprintln!("warning: skipping template `{name}`: {error:#}"),
            }
        }
    }
    Ok(found.into_values().collect())
}

/// The template `name`: `project`'s own, else the global one.
pub fn resolve(ctx: &Ctx, project: Option<&str>, name: &str) -> Result<Template> {
    project::validate_slug(name)?;
    for scope in project.map(Scope::of).into_iter().chain([Scope::Global]) {
        let dir = scope_dir(ctx, &scope)?.join(name);
        if std::fs::symlink_metadata(&dir).is_ok() {
            return load(&dir, name, scope);
        }
    }
    bail!(
        "there is no template `{name}`; `template list{}` shows them",
        project
            .map(|p| format!(" --project {p}"))
            .unwrap_or_default()
    )
}

fn load(dir: &Path, name: &str, scope: Scope) -> Result<Template> {
    if !std::fs::symlink_metadata(dir)?.file_type().is_dir() {
        bail!("{} must be a regular directory", dir.display());
    }
    let text = organizations::read_optional_text(&dir.join(SPEC), true)?;
    if text.trim().is_empty() {
        bail!("{} has no {SPEC}", dir.display());
    }
    let spec = toml::from_str(&text).with_context(|| format!("{SPEC} does not parse"))?;
    Ok(Template {
        name: name.to_string(),
        scope,
        dir: dir.to_path_buf(),
        spec,
    })
}

impl Template {
    pub fn rules(&self) -> Result<String> {
        organizations::read_optional_text(&self.dir.join(RULES), true)
    }

    pub fn memory(&self) -> Result<Memory> {
        organizations::read_memory(&self.dir, &format!("template {}", self.name))
    }

    /// Gives a new node what its caller left unset: the role unless `role`
    /// chose one, its spawn permission while the role is the template's, the
    /// profile as a fallback (after `--profile`, `--harness` and a task's
    /// owner), the rules (unless given) and the memory it starts with.
    pub fn fill(&self, args: &mut StartArgs, role: Option<NodeRole>) -> Result<()> {
        let node = &mut args.node;
        node.role = role.unwrap_or(self.spec.role);
        if node.can_spawn.is_none() && node.role == self.spec.role {
            node.can_spawn = Some(self.spec.can_spawn);
        }
        if !self.spec.profile.is_empty() {
            node.template_profile = Some(self.spec.profile.clone());
        }
        if node.rules.trim().is_empty() {
            node.rules = self.rules()?;
        }
        node.memory = self.memory()?;
        Ok(())
    }

    /// One `template list --json` entry (docs/json.md). `harness`, `model`
    /// and `reasoning_effort` are the named profile's, empty when it is
    /// unset or gone.
    pub fn json(&self, profiles: &crate::profiles::Config) -> Result<serde_json::Value> {
        let entry = profiles
            .get(&self.spec.profile)
            .map(|p| p.entry)
            .unwrap_or_default();
        Ok(serde_json::json!({
            "name": self.name,
            "scope": self.scope.word(),
            "project": self.scope.slug(),
            "description": self.spec.description,
            "role": self.spec.role,
            "can_spawn": self.spec.can_spawn,
            "profile": self.spec.profile,
            "harness": entry.agent,
            "model": entry.model,
            "reasoning_effort": entry.effort,
            "permission_profile": "",
            "rules_chars": self.rules()?.trim().chars().count(),
            "memory_chars": self.memory()?.chars(),
            "updated": self.spec.updated,
            "dir": self.dir.to_string_lossy(),
        }))
    }

    /// `template show --json`: the list entry with the rules and memory.
    pub fn show_json(&self, profiles: &crate::profiles::Config) -> Result<serde_json::Value> {
        let mut template = self.json(profiles)?;
        let memory = self.memory()?;
        template["rules"] = self.rules()?.into();
        template["memory"] = memory.index.into();
        template["memory_files"] = memory
            .files
            .into_iter()
            .map(|(name, text)| serde_json::json!({ "name": name, "text": text }))
            .collect();
        Ok(serde_json::json!({
            "schema_version": crate::threads::JSON_SCHEMA_VERSION,
            "template": template,
        }))
    }

    fn profile_shown(&self) -> &str {
        match self.spec.profile.as_str() {
            "" => "inherited",
            profile => profile,
        }
    }

    /// One `template list` line: name, scope, role, profile, description.
    pub fn line(&self) -> String {
        format!(
            "{}\t{}\t{}\t{}\t{}",
            self.name,
            self.scope.word(),
            self.spec.role.as_str(),
            self.profile_shown(),
            self.spec.description
        )
    }

    /// `template show`: the template, its rules and its memory.
    pub fn show_text(&self) -> Result<String> {
        let spec = &self.spec;
        let mut text = format!(
            "{} ({}{})\nrole: {}{}\nprofile: {}\n",
            self.name,
            self.scope.word(),
            self.scope
                .slug()
                .map(|s| format!(" {s}"))
                .unwrap_or_default(),
            spec.role.as_str(),
            match (spec.role, spec.can_spawn) {
                (NodeRole::Coordinator, false) => ", cannot spawn",
                _ => "",
            },
            self.profile_shown(),
        );
        if !spec.description.is_empty() {
            text.push_str(&format!("description: {}\n", spec.description));
        }
        text.push_str(&format!("dir: {}\n\n## Rules\n\n", self.dir.display()));
        let rules = self.rules()?;
        text.push_str(if rules.trim().is_empty() {
            "(none)\n"
        } else {
            &rules
        });
        let memory = self.memory()?;
        text.push_str("\n## Memory\n\n");
        text.push_str(if memory.index.trim().is_empty() {
            "(none)\n"
        } else {
            &memory.index
        });
        for (name, body) in &memory.files {
            text.push_str(&format!("\n### memory/{name}\n\n{body}"));
        }
        Ok(text)
    }
}

/// Saves node `id` of `slug` as template `name`: its role, spawn
/// permission, profile name, rules and memory. It goes to the project unless
/// `global`; an existing one is replaced only with `force`, keeping its
/// creation time.
pub fn save(
    ctx: &Ctx,
    slug: &str,
    id: &str,
    name: &str,
    global: bool,
    description: &str,
    force: bool,
) -> Result<Template> {
    project::validate_slug(name)?;
    let project = Project::load(&ctx.root, slug)?;
    let node = thread::load(&project, id)?;
    let rules = organizations::node_rules(&project, id)?;
    let rules = organizations::instruction_rules(&rules);
    let memory = organizations::node_memory(&project, id)?;
    let scope = if global {
        Scope::Global
    } else {
        Scope::Project(project.slug.clone())
    };
    let parent = scope_dir(ctx, &scope)?;
    let dir = parent.join(name);
    let previous = match std::fs::symlink_metadata(&dir) {
        Ok(_) if !force => bail!("template `{name}` exists; --force replaces it"),
        // A broken template can be replaced too.
        Ok(_) => load(&dir, name, scope.clone()).ok(),
        Err(_) => None,
    };
    let now = project::now();
    let spec = Spec {
        role: node.role,
        can_spawn: node.can_spawn,
        profile: [node.profile, node.agent]
            .into_iter()
            .find(|p| !p.is_empty())
            .unwrap_or_default(),
        description: description.trim().to_string(),
        created: previous
            .map(|t| t.spec.created)
            .filter(|c| !c.is_empty())
            .unwrap_or_else(|| now.clone()),
        updated: now,
    };

    // Written whole beside the old one, then swapped in.
    std::fs::create_dir_all(&parent)
        .with_context(|| format!("could not create {}", parent.display()))?;
    let staging = parent.join(format!(".{name}.saving"));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir(&staging)
        .with_context(|| format!("could not create {}", staging.display()))?;
    let written = (|| -> Result<()> {
        project::write_atomic(&staging.join(SPEC), toml::to_string(&spec)?.as_bytes())?;
        if !rules.is_empty() {
            project::write_atomic(&staging.join(RULES), format!("{rules}\n").as_bytes())?;
        }
        organizations::write_memory(&staging, &memory)?;
        if std::fs::symlink_metadata(&dir).is_ok() {
            std::fs::remove_dir_all(&dir)
                .with_context(|| format!("could not replace {}", dir.display()))?;
        }
        std::fs::rename(&staging, &dir)
            .with_context(|| format!("could not move the template into {}", dir.display()))
    })();
    if written.is_err() {
        let _ = std::fs::remove_dir_all(&staging);
    }
    written?;
    load(&dir, name, scope)
}

/// Deletes template `name` from `project`, or the global one without a
/// project; never the other scope's.
pub fn delete(ctx: &Ctx, project: Option<&str>, name: &str) -> Result<()> {
    project::validate_slug(name)?;
    let scope = project.map_or(Scope::Global, Scope::of);
    let dir = scope_dir(ctx, &scope)?.join(name);
    if std::fs::symlink_metadata(&dir).is_err() {
        bail!(
            "there is no {} template `{name}`",
            scope
                .slug()
                .map_or("global".to_string(), |slug| format!("`{slug}`"))
        );
    }
    load(&dir, name, scope)?;
    std::fs::remove_dir_all(&dir).with_context(|| format!("could not delete {}", dir.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_profile::ProfileOverrides;
    use crate::organizations::{CreateNode, NodeRequest, ROOT_ID};
    use crate::scenarios::World;
    use crate::thread::Kind;

    /// A coordinator node `t-0001` on profile `deep` with rules and memory.
    fn node(world: &World) -> Project {
        let project = world.project("demo", "a.sock");
        let node = organizations::create_node(
            &project,
            &CreateNode {
                request: NodeRequest {
                    role: NodeRole::Coordinator,
                    can_spawn: Some(false),
                    profile: ProfileOverrides {
                        harness: Some("codex".into()),
                        model: Some("gpt-5.6".into()),
                        ..ProfileOverrides::default()
                    },
                    rules: "Keep changes accessible.".into(),
                    ..NodeRequest::default()
                },
                title: "Frontend".into(),
                kind: Kind::Tab,
                repo: String::new(),
                machine: String::new(),
                base: String::new(),
                task: "Coordinate the frontend.".into(),
            },
        )
        .unwrap();
        thread::update(&project, &node.id, |t| t.profile = "deep".into()).unwrap();
        let scope = organizations::node_scope_dir(&project, &node.id);
        std::fs::write(scope.join("MEMORY.md"), "# Node memory\n\n- Use pnpm.\n").unwrap();
        std::fs::write(scope.join("memory/stack.md"), "React 19.\n").unwrap();
        project
    }

    fn start_args() -> StartArgs {
        StartArgs {
            title: "Next".into(),
            repo: None,
            machine: None,
            profile: None,
            kind: None,
            base: None,
            task: "Do it.".into(),
            node: NodeRequest::default(),
        }
    }

    #[test]
    fn a_saved_node_gives_a_new_node_its_role_profile_rules_and_memory() {
        let world = World::new();
        node(&world);
        let ctx = world.ctx();
        let saved = save(&ctx, "demo", "t-0001", "frontend", false, "UI work", false).unwrap();
        assert_eq!(saved.scope, Scope::Project("demo".into()));
        assert_eq!(
            saved.spec,
            Spec {
                role: NodeRole::Coordinator,
                can_spawn: false,
                profile: "deep".into(),
                description: "UI work".into(),
                ..saved.spec.clone()
            }
        );
        // The node's own model flag is not a profile, so it is not kept.
        let toml = std::fs::read_to_string(saved.dir.join(SPEC)).unwrap();
        assert!(!toml.contains("gpt-5.6"), "{toml}");

        let mut args = start_args();
        resolve(&ctx, Some("demo"), "frontend")
            .unwrap()
            .fill(&mut args, None)
            .unwrap();
        assert_eq!(args.node.template_profile.as_deref(), Some("deep"));
        assert_eq!(args.node.role, NodeRole::Coordinator);
        assert_eq!(args.node.can_spawn, Some(false));
        assert_eq!(args.node.rules, "Keep changes accessible.\n");
        assert_eq!(
            args.node.memory,
            Memory {
                index: "# Node memory\n\n- Use pnpm.\n".into(),
                files: vec![("stack.md".into(), "React 19.\n".into())],
            }
        );
    }

    #[test]
    fn flags_given_with_a_template_win() {
        let world = World::new();
        node(&world);
        let ctx = world.ctx();
        let template = save(&ctx, "demo", "t-0001", "frontend", false, "", false).unwrap();

        let mut args = start_args();
        args.node.role = NodeRole::Worker;
        args.node.profile.harness = Some("claude".into());
        args.node.rules = "Own rules.".into();
        template.fill(&mut args, Some(NodeRole::Worker)).unwrap();
        assert_eq!(args.node.role, NodeRole::Worker);
        // A worker does not take a coordinator's spawn permission.
        assert_eq!(args.node.can_spawn, None);
        assert_eq!(args.node.rules, "Own rules.");
        assert_eq!(args.node.memory.files.len(), 1);
        // The template's profile is only a fallback: `--profile` and
        // `--harness` stay as given and win in `threads::start`.
        assert_eq!(args.profile, None);
        assert_eq!(args.node.profile.harness.as_deref(), Some("claude"));
    }

    #[test]
    fn a_project_template_wins_over_a_global_one_of_the_same_name() {
        let world = World::new();
        node(&world);
        let ctx = world.ctx();
        save(&ctx, "demo", "t-0001", "frontend", true, "global", false).unwrap();
        save(&ctx, "demo", "t-0001", "frontend", false, "project", false).unwrap();
        save(&ctx, "demo", "t-0001", "shared", true, "", false).unwrap();
        assert!(
            ctx.config_dir
                .join("templates/frontend")
                .join(SPEC)
                .is_file()
        );

        let listed = list(&ctx, Some("demo")).unwrap();
        let shown: Vec<_> = listed
            .iter()
            .map(|t| (t.name.as_str(), t.spec.description.as_str()))
            .collect();
        assert_eq!(shown, [("frontend", "project"), ("shared", "")]);
        let global: Vec<_> = list(&ctx, None)
            .unwrap()
            .into_iter()
            .map(|t| t.scope)
            .collect();
        assert_eq!(global, [Scope::Global, Scope::Global]);
        assert_eq!(
            resolve(&ctx, None, "frontend").unwrap().spec.description,
            "global"
        );

        // Deleting the project's own leaves the global one.
        delete(&ctx, Some("demo"), "frontend").unwrap();
        assert_eq!(
            resolve(&ctx, Some("demo"), "frontend").unwrap().scope,
            Scope::Global
        );
        let error = delete(&ctx, Some("demo"), "frontend").unwrap_err();
        assert_eq!(error.to_string(), "there is no `demo` template `frontend`");
    }

    #[test]
    fn saving_over_a_template_needs_force_and_keeps_its_creation_time() {
        let world = World::new();
        let project = node(&world);
        let ctx = world.ctx();
        let first = save(&ctx, "demo", "t-0001", "frontend", false, "", false).unwrap();
        let error = save(&ctx, "demo", "t-0001", "frontend", false, "", false).unwrap_err();
        assert_eq!(
            error.to_string(),
            "template `frontend` exists; --force replaces it"
        );

        organizations::set_node_rules(&project, "t-0001", "Newer rules.").unwrap();
        std::fs::remove_file(
            organizations::node_scope_dir(&project, "t-0001").join("memory/stack.md"),
        )
        .unwrap();
        let replaced = save(&ctx, "demo", "t-0001", "frontend", false, "", true).unwrap();
        assert_eq!(replaced.spec.created, first.spec.created);
        assert_eq!(replaced.rules().unwrap(), "Newer rules.\n");
        assert!(replaced.memory().unwrap().files.is_empty());
        assert!(!replaced.dir.with_file_name(".frontend.saving").exists());
    }

    #[test]
    fn a_new_node_without_rules_or_memory_saves_an_empty_template() {
        let world = World::new();
        let project = world.project("demo", "a.sock");
        organizations::create_node(
            &project,
            &CreateNode {
                request: NodeRequest {
                    parent_id: ROOT_ID.into(),
                    ..NodeRequest::default()
                },
                title: "Plain".into(),
                kind: Kind::Tab,
                repo: String::new(),
                machine: String::new(),
                base: String::new(),
                task: "task".into(),
            },
        )
        .unwrap();
        let ctx = world.ctx();
        let template = save(&ctx, "demo", "t-0001", "plain", false, "", false).unwrap();
        assert_eq!(template.rules().unwrap(), "");
        assert_eq!(template.memory().unwrap(), Memory::default());
        let row = template.json(&crate::profiles::Config::default()).unwrap();
        assert_eq!(row["rules_chars"], 0);
        assert_eq!(row["memory_chars"], 0);
    }

    #[test]
    fn memory_a_brief_would_omit_is_refused_and_nothing_is_saved() {
        let world = World::new();
        let project = node(&world);
        std::fs::write(
            organizations::node_scope_dir(&project, "t-0001").join("memory/big.md"),
            "x".repeat(organizations::MAX_SCOPED_MEMORY_FILE_BYTES + 1),
        )
        .unwrap();
        let ctx = world.ctx();
        let error = save(&ctx, "demo", "t-0001", "frontend", false, "", false)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("nodes/t-0001/memory/big.md: exceeds"),
            "{error}"
        );
        assert!(list(&ctx, Some("demo")).unwrap().is_empty());
    }

    #[test]
    fn a_broken_template_is_skipped_by_list_and_an_error_by_name() {
        let world = World::new();
        node(&world);
        let ctx = world.ctx();
        save(&ctx, "demo", "t-0001", "good", false, "", false).unwrap();
        let broken = ctx.root.join("demo/templates/broken");
        std::fs::create_dir_all(&broken).unwrap();
        std::fs::write(broken.join(SPEC), "role = [").unwrap();

        let names: Vec<_> = list(&ctx, Some("demo"))
            .unwrap()
            .into_iter()
            .map(|t| t.name)
            .collect();
        assert_eq!(names, ["good"]);
        let error = resolve(&ctx, Some("demo"), "broken").unwrap_err();
        assert!(format!("{error:#}").contains("does not parse"), "{error:#}");
        // `--force` replaces it.
        save(&ctx, "demo", "t-0001", "broken", false, "", true).unwrap();
        assert!(resolve(&ctx, Some("demo"), "broken").is_ok());
    }
}

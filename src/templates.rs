//! Reusable node templates, stored globally or inside a project.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::organizations;
use crate::project::{self, Project};
use crate::thread::{self, NodeRole};

pub const TEMPLATES_DIR: &str = ".templates";
pub const PROJECT_TEMPLATES_DIR: &str = "templates";
pub const MAX_TEMPLATE_MEMORY_BYTES: usize = 8 * 1024;
const MEMORY_HEADER: &str = "# Template memory";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(default)]
pub struct TemplateSpec {
    pub name: String,
    pub description: String,
    pub role: NodeRole,
    pub can_spawn: bool,
    pub harness: String,
    pub model: String,
    pub reasoning_effort: String,
    pub permission_profile: String,
    pub raw_agent_args: Vec<String>,
    pub created: String,
    pub updated: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    Global,
    Project,
}

#[derive(Debug)]
pub struct Template {
    pub spec: TemplateSpec,
    pub scope: Scope,
    pub project: String,
    pub dir: PathBuf,
}

pub struct SaveArgs {
    pub name: String,
    pub project: Option<String>,
    pub spec: TemplateSpec,
    pub rules: String,
    pub force: bool,
}

pub fn dir_for(root: &Path, project: Option<&str>, name: &str) -> PathBuf {
    match project {
        Some(project) => root.join(project).join(PROJECT_TEMPLATES_DIR).join(name),
        None => root.join(TEMPLATES_DIR).join(name),
    }
}

pub fn list(root: &Path, project: Option<&str>) -> Result<Vec<Template>> {
    let mut templates = BTreeMap::new();
    if let Some(project_slug) = project {
        let project = Project::load(root, project_slug)?;
        for template in read_scope(
            &project.dir().join(PROJECT_TEMPLATES_DIR),
            Scope::Project,
            &project.slug,
        )? {
            templates.insert(template.spec.name.clone(), template);
        }
    }
    for template in read_scope(&root.join(TEMPLATES_DIR), Scope::Global, "")? {
        templates
            .entry(template.spec.name.clone())
            .or_insert(template);
    }
    Ok(templates.into_values().collect())
}

pub fn resolve(root: &Path, project: Option<&str>, name: &str) -> Result<Template> {
    project::validate_slug(name)?;
    if let Some(project_slug) = project {
        let project = Project::load(root, project_slug)?;
        if let Some(template) = read_template(
            &project.dir().join(PROJECT_TEMPLATES_DIR).join(name),
            Scope::Project,
            &project.slug,
        )? {
            return Ok(template);
        }
    }
    if let Some(template) = read_template(&root.join(TEMPLATES_DIR).join(name), Scope::Global, "")?
    {
        return Ok(template);
    }
    bail!("template `{name}` not found")
}

pub fn rules(template: &Template) -> Result<String> {
    read_optional_text(&template.dir.join("RULES.md"))
}

pub fn memory(template: &Template) -> Result<String> {
    read_optional_text(&template.dir.join("MEMORY.md"))
}

pub fn save(root: &Path, args: SaveArgs) -> Result<Template> {
    project::validate_slug(&args.name)?;
    if let Some(project_slug) = args.project.as_deref() {
        Project::load(root, project_slug)?;
    }
    if args.rules.len() > organizations::MAX_NODE_RULES_BYTES {
        bail!(
            "rules are over {} bytes",
            organizations::MAX_NODE_RULES_BYTES
        );
    }
    if args.spec.role == NodeRole::Worker && args.spec.can_spawn {
        bail!("worker nodes cannot spawn children");
    }

    let dir = dir_for(root, args.project.as_deref(), &args.name);
    let existed = std::fs::symlink_metadata(&dir).is_ok();
    if existed && !args.force {
        bail!(
            "template `{}` exists; pass --force to replace it",
            args.name
        );
    }

    let previous = if existed {
        read_spec(&dir.join("TEMPLATE.toml")).ok()
    } else {
        None
    };
    let now = project::now();
    let mut spec = args.spec;
    spec.name = args.name;
    spec.created = previous
        .as_ref()
        .map(|previous| previous.created.clone())
        .unwrap_or_else(|| now.clone());
    spec.updated = now;

    std::fs::create_dir_all(&dir)
        .with_context(|| format!("could not create template directory {}", dir.display()))?;
    project::write_atomic(
        &dir.join("TEMPLATE.toml"),
        toml::to_string(&spec)?.as_bytes(),
    )?;
    project::write_atomic(&dir.join("RULES.md"), args.rules.as_bytes())?;
    let memory_path = dir.join("MEMORY.md");
    if !memory_path.exists() {
        project::write_atomic(&memory_path, format!("{MEMORY_HEADER}\n").as_bytes())?;
    }
    read_template(
        &dir,
        scope_for(args.project.as_deref()),
        args.project.as_deref().unwrap_or(""),
    )?
    .context("saved template could not be loaded")
}

pub fn spec_from_node(
    root: &Path,
    slug: &str,
    id: &str,
    name: &str,
    description: &str,
) -> Result<(TemplateSpec, String)> {
    project::validate_slug(name)?;
    let project = Project::load(root, slug)?;
    let node = thread::load(&project, id)?;
    let instructions =
        read_optional_text(&organizations::node_scope_dir(&project, id).join("INSTRUCTIONS.md"))?;
    let rules = instructions
        .strip_prefix("# Node instructions")
        .unwrap_or(&instructions)
        .trim_start_matches(['\r', '\n'])
        .to_string();
    let rules = if rules.trim() == "Standing instructions for this node and its descendants." {
        String::new()
    } else {
        rules
    };
    Ok((
        TemplateSpec {
            name: name.to_string(),
            description: description.to_string(),
            role: node.role,
            can_spawn: node.can_spawn,
            harness: node.agent,
            model: node.model,
            reasoning_effort: node.reasoning_effort,
            permission_profile: node.permission_profile,
            raw_agent_args: node.raw_agent_args,
            created: String::new(),
            updated: String::new(),
        },
        rules,
    ))
}

/// The memory text without the file's own heading; empty when nothing was learned yet.
pub fn memory_body(memory: &str) -> &str {
    let memory = memory.trim();
    memory.strip_prefix(MEMORY_HEADER).unwrap_or(memory).trim()
}

pub fn set_memory(template: &Template, text: &str) -> Result<()> {
    if text.len() > MAX_TEMPLATE_MEMORY_BYTES {
        bail!("template memory is over {MAX_TEMPLATE_MEMORY_BYTES} bytes");
    }
    project::write_atomic(&template.dir.join("MEMORY.md"), text.as_bytes())
}

pub fn delete(root: &Path, project_slug: Option<&str>, name: &str) -> Result<()> {
    project::validate_slug(name)?;
    if let Some(project_slug) = project_slug {
        Project::load(root, project_slug)?;
    }
    let dir = dir_for(root, project_slug, name);
    let metadata =
        std::fs::symlink_metadata(&dir).with_context(|| format!("template `{name}` not found"))?;
    if !metadata.file_type().is_dir() || !dir.join("TEMPLATE.toml").is_file() {
        bail!("{} is not a template directory", dir.display());
    }
    std::fs::remove_dir_all(&dir).with_context(|| format!("could not delete template `{name}`"))
}

fn read_scope(dir: &Path, scope: Scope, project: &str) -> Result<Vec<Template>> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(error).with_context(|| format!("could not read {}", dir.display()));
        }
    };
    let mut templates = Vec::new();
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        if let Some(template) = read_template(&entry.path(), scope, project)? {
            templates.push(template);
        }
    }
    Ok(templates)
}

fn read_template(dir: &Path, scope: Scope, project: &str) -> Result<Option<Template>> {
    let spec_path = dir.join("TEMPLATE.toml");
    if !spec_path.is_file() {
        return Ok(None);
    }
    let spec = match read_spec(&spec_path) {
        Ok(spec) => spec,
        Err(_) => return Ok(None),
    };
    let Some(name) = dir.file_name().and_then(|name| name.to_str()) else {
        return Ok(None);
    };
    if project::validate_slug(name).is_err() || spec.name != name {
        return Ok(None);
    }
    Ok(Some(Template {
        spec,
        scope,
        project: project.to_string(),
        dir: dir.to_path_buf(),
    }))
}

fn read_spec(path: &Path) -> Result<TemplateSpec> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("could not read {}", path.display()))?;
    toml::from_str(&text).with_context(|| format!("{} does not parse", path.display()))
}

fn read_optional_text(path: &Path) -> Result<String> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(text),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(error) => Err(error).with_context(|| format!("could not read {}", path.display())),
    }
}

fn scope_for(project: Option<&str>) -> Scope {
    if project.is_some() {
        Scope::Project
    } else {
        Scope::Global
    }
}

#[cfg(test)]
mod tests {
    use crate::agent_profile::ProfileOverrides;
    use crate::organizations::{self, CreateNode, NodeRequest, ROOT_ID};
    use crate::thread::{Kind, NodeRole};

    use super::*;

    #[test]
    fn memory_body_ignores_the_heading_of_an_empty_memory() {
        assert_eq!(memory_body("# Template memory\n"), "");
        assert_eq!(
            memory_body("# Template memory\n\n- Use pnpm.\n"),
            "- Use pnpm."
        );
        assert_eq!(memory_body("- Use pnpm."), "- Use pnpm.");
    }

    fn spec(name: &str, role: NodeRole) -> TemplateSpec {
        TemplateSpec {
            name: name.to_string(),
            role,
            can_spawn: role == NodeRole::Coordinator,
            ..TemplateSpec::default()
        }
    }

    fn save_template(
        root: &Path,
        name: &str,
        project: Option<&str>,
        role: NodeRole,
        rules: &str,
        force: bool,
    ) -> Result<Template> {
        save(
            root,
            SaveArgs {
                name: name.to_string(),
                project: project.map(str::to_string),
                spec: spec(name, role),
                rules: rules.to_string(),
                force,
            },
        )
    }

    #[test]
    fn save_then_list_and_resolve_global() {
        let root = tempfile::tempdir().unwrap();
        save_template(
            root.path(),
            "frontend",
            None,
            NodeRole::Coordinator,
            "Coordinate frontend work.",
            false,
        )
        .unwrap();

        let listed = list(root.path(), None).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].spec.name, "frontend");
        assert_eq!(listed[0].scope, Scope::Global);
        assert_eq!(rules(&listed[0]).unwrap(), "Coordinate frontend work.");
        assert_eq!(
            resolve(root.path(), None, "frontend").unwrap().dir,
            dir_for(root.path(), None, "frontend")
        );
    }

    #[test]
    fn project_template_overrides_global_with_same_name() {
        let root = tempfile::tempdir().unwrap();
        project::create(root.path(), "Demo", "", vec![]).unwrap();
        save_template(
            root.path(),
            "frontend",
            None,
            NodeRole::Coordinator,
            "Global rules.",
            false,
        )
        .unwrap();
        save_template(
            root.path(),
            "frontend",
            Some("demo"),
            NodeRole::Worker,
            "Project rules.",
            false,
        )
        .unwrap();

        let listed = list(root.path(), Some("demo")).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].scope, Scope::Project);
        assert_eq!(listed[0].project, "demo");
        assert_eq!(rules(&listed[0]).unwrap(), "Project rules.");
        assert_eq!(
            resolve(root.path(), Some("demo"), "frontend")
                .unwrap()
                .scope,
            Scope::Project
        );
        assert_eq!(
            resolve(root.path(), None, "frontend").unwrap().scope,
            Scope::Global
        );
    }

    #[test]
    fn save_refuses_existing_without_force_and_force_keeps_memory_and_created() {
        let root = tempfile::tempdir().unwrap();
        let first = save_template(
            root.path(),
            "frontend",
            None,
            NodeRole::Coordinator,
            "First rules.",
            false,
        )
        .unwrap();
        set_memory(&first, "Keep this memory.").unwrap();
        let created = first.spec.created.clone();
        let error = save_template(
            root.path(),
            "frontend",
            None,
            NodeRole::Coordinator,
            "Replacement rules.",
            false,
        )
        .unwrap_err();
        assert_eq!(
            error.to_string(),
            "template \u{60}frontend\u{60} exists; pass --force to replace it"
        );

        let replaced = save_template(
            root.path(),
            "frontend",
            None,
            NodeRole::Coordinator,
            "Replacement rules.",
            true,
        )
        .unwrap();
        assert_eq!(replaced.spec.created, created);
        assert_eq!(rules(&replaced).unwrap(), "Replacement rules.");
        assert_eq!(memory(&replaced).unwrap(), "Keep this memory.");
    }

    #[test]
    fn worker_with_can_spawn_is_rejected() {
        let root = tempfile::tempdir().unwrap();
        let mut worker = spec("leaf", NodeRole::Worker);
        worker.can_spawn = true;
        let error = save(
            root.path(),
            SaveArgs {
                name: "leaf".into(),
                project: None,
                spec: worker,
                rules: String::new(),
                force: false,
            },
        )
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("worker nodes cannot spawn children")
        );
    }

    #[test]
    fn rules_over_limit_are_rejected() {
        let root = tempfile::tempdir().unwrap();
        let error = save_template(
            root.path(),
            "large",
            None,
            NodeRole::Coordinator,
            &"x".repeat(organizations::MAX_NODE_RULES_BYTES + 1),
            false,
        )
        .unwrap_err();
        assert!(error.to_string().contains("rules are over"));
        assert!(!dir_for(root.path(), None, "large").exists());
    }

    #[test]
    fn spec_from_node_copies_role_profile_and_rules_without_header() {
        let root = tempfile::tempdir().unwrap();
        let project = project::create(root.path(), "Demo", "", vec![]).unwrap();
        let node = organizations::create_node(
            &project,
            &CreateNode {
                request: NodeRequest {
                    parent_id: ROOT_ID.into(),
                    role: NodeRole::Coordinator,
                    can_spawn: Some(false),
                    profile: ProfileOverrides {
                        harness: Some("codex".into()),
                        model: Some("gpt-5.6".into()),
                        reasoning_effort: Some("high".into()),
                        permission_profile: Some("workspace-write".into()),
                        raw_agent_args: vec!["--color=never".into()],
                    },
                },
                title: "Frontend".into(),
                kind: Kind::Tab,
                repo: String::new(),
                machine: String::new(),
                base: String::new(),
                task: "Implement the frontend.".into(),
                rules: "Keep changes accessible.".into(),
                template: String::new(),
            },
        )
        .unwrap();

        let (copied, rules) =
            spec_from_node(root.path(), "demo", &node.id, "frontend", "Frontend work").unwrap();
        assert_eq!(copied.name, "frontend");
        assert_eq!(copied.description, "Frontend work");
        assert_eq!(copied.role, NodeRole::Coordinator);
        assert!(!copied.can_spawn);
        assert_eq!(copied.harness, "codex");
        assert_eq!(copied.model, "gpt-5.6");
        assert_eq!(copied.reasoning_effort, "high");
        assert_eq!(copied.permission_profile, "workspace-write");
        assert_eq!(copied.raw_agent_args, ["--color=never"]);
        assert_eq!(rules, "Keep changes accessible.\n");
    }

    #[test]
    fn delete_requires_template_toml_and_does_not_fall_back_to_global() {
        let root = tempfile::tempdir().unwrap();
        project::create(root.path(), "Demo", "", vec![]).unwrap();
        save_template(
            root.path(),
            "frontend",
            None,
            NodeRole::Coordinator,
            "Global rules.",
            false,
        )
        .unwrap();
        let project_dir = dir_for(root.path(), Some("demo"), "frontend");
        std::fs::create_dir_all(&project_dir).unwrap();
        std::fs::write(project_dir.join("RULES.md"), "not a template").unwrap();

        assert!(delete(root.path(), Some("demo"), "frontend").is_err());
        assert!(project_dir.exists());
        assert_eq!(
            resolve(root.path(), None, "frontend").unwrap().scope,
            Scope::Global
        );
    }

    #[test]
    fn invalid_template_folder_is_skipped_by_list() {
        let root = tempfile::tempdir().unwrap();
        save_template(root.path(), "valid", None, NodeRole::Coordinator, "", false).unwrap();
        let invalid = dir_for(root.path(), None, "invalid");
        std::fs::create_dir_all(&invalid).unwrap();
        std::fs::write(invalid.join("TEMPLATE.toml"), "name = [").unwrap();

        let listed = list(root.path(), None).unwrap();
        assert_eq!(
            listed
                .iter()
                .map(|template| template.spec.name.as_str())
                .collect::<Vec<_>>(),
            ["valid"]
        );
    }

    #[test]
    fn set_memory_rejects_values_over_the_limit() {
        let root = tempfile::tempdir().unwrap();
        let template = save_template(
            root.path(),
            "memory",
            None,
            NodeRole::Coordinator,
            "",
            false,
        )
        .unwrap();
        let error = set_memory(&template, &"x".repeat(MAX_TEMPLATE_MEMORY_BYTES + 1)).unwrap_err();
        assert!(error.to_string().contains("template memory is over"));
    }
}

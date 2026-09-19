//! The virtual project root and its recursive coordinator/worker nodes.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs::{File, OpenOptions};
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::agent_profile::{AgentProfile, ProfileOverrides};
use crate::project::{self, Project};
use crate::thread::{self, Kind, NodeRole, Status, Thread};

pub const ROOT_ID: &str = "root";
pub const MAX_SCOPED_MEMORY_FILE_BYTES: usize = 8 * 1024;
pub const MAX_SCOPED_MEMORY_TOTAL_BYTES: usize = 32_000;
pub const MAX_SCOPED_MEMORY_FILES: usize = 64;
const MAX_MEMORY_NAMES_PER_DIRECTORY: usize = MAX_SCOPED_MEMORY_FILES;
const MAX_MEMORY_OMISSION_DETAILS: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeRequest {
    pub parent_id: String,
    pub role: NodeRole,
    pub can_spawn: Option<bool>,
    pub profile: ProfileOverrides,
}

impl Default for NodeRequest {
    fn default() -> Self {
        Self {
            parent_id: ROOT_ID.into(),
            role: NodeRole::Worker,
            can_spawn: None,
            profile: ProfileOverrides::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateNode {
    pub request: NodeRequest,
    pub title: String,
    pub kind: Kind,
    pub repo: String,
    pub machine: String,
    pub base: String,
    pub task: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeEntry {
    pub thread: Thread,
    pub depth: usize,
    /// Root is 0; descendants use stable preorder numbers beginning at 1.
    pub tree_order: usize,
    pub is_last: bool,
    /// Unicode branch characters for a terminal tree.
    pub prefix: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopedContext {
    pub instructions: String,
    pub memory_index: String,
    /// (display path, contents), in deterministic root-to-target order.
    pub memory_files: Vec<(String, String)>,
}

pub fn tree(project: &Project) -> Result<Vec<TreeEntry>> {
    tree_from(&thread::list(project))
}

pub fn tree_from(threads: &[Thread]) -> Result<Vec<TreeEntry>> {
    let by_id = validate_tree(threads)?;
    let mut children: BTreeMap<&str, Vec<&Thread>> = BTreeMap::new();
    for record in threads {
        children.entry(parent_id(record)).or_default().push(record);
    }
    for records in children.values_mut() {
        records.sort_by(|a, b| compare_ids(&a.id, &b.id));
    }

    let mut ordered = Vec::with_capacity(threads.len());
    let mut visited = HashSet::with_capacity(threads.len());
    let root_children = children.get(ROOT_ID).map(Vec::as_slice).unwrap_or_default();
    let mut stack = Vec::new();
    for (index, record) in root_children.iter().enumerate().rev() {
        stack.push((
            *record,
            1usize,
            index + 1 == root_children.len(),
            Vec::<bool>::new(),
        ));
    }

    while let Some((record, depth, is_last, ancestors_last)) = stack.pop() {
        if !visited.insert(record.id.as_str()) {
            bail!("organization hierarchy contains a cycle at `{}`", record.id);
        }
        let mut prefix = String::new();
        for ancestor_is_last in &ancestors_last {
            prefix.push_str(if *ancestor_is_last { "   " } else { "│  " });
        }
        prefix.push_str(if is_last { "└─ " } else { "├─ " });
        ordered.push(TreeEntry {
            thread: record.clone(),
            depth,
            tree_order: ordered.len() + 1,
            is_last,
            prefix,
        });

        let next_ancestors = ancestors_last
            .iter()
            .copied()
            .chain([is_last])
            .collect::<Vec<_>>();
        let node_children = children
            .get(record.id.as_str())
            .map(Vec::as_slice)
            .unwrap_or_default();
        for (index, child) in node_children.iter().enumerate().rev() {
            stack.push((
                *child,
                depth + 1,
                index + 1 == node_children.len(),
                next_ancestors.clone(),
            ));
        }
    }

    if visited.len() != by_id.len() {
        bail!("organization hierarchy contains nodes unreachable from `{ROOT_ID}`");
    }
    Ok(ordered)
}

fn validate_tree(threads: &[Thread]) -> Result<HashMap<&str, &Thread>> {
    let mut by_id = HashMap::with_capacity(threads.len());
    for record in threads {
        thread::validate_id(&record.id)?;
        if by_id.insert(record.id.as_str(), record).is_some() {
            bail!(
                "organization hierarchy contains duplicate node `{}`",
                record.id
            );
        }
    }

    for record in threads {
        let parent = parent_id(record);
        if parent == record.id {
            bail!("node `{}` cannot be its own parent", record.id);
        }
        if parent != ROOT_ID {
            thread::validate_id(parent)?;
            if !by_id.contains_key(parent) {
                bail!("node `{}` has missing parent `{parent}`", record.id);
            }
        }
    }

    let mut complete = HashSet::with_capacity(threads.len());
    let mut ids: Vec<&str> = by_id.keys().copied().collect();
    ids.sort_by(|a, b| compare_ids(a, b));
    for id in ids {
        let mut path = HashSet::new();
        let mut chain = Vec::new();
        let mut current = id;
        while current != ROOT_ID && !complete.contains(current) {
            if !path.insert(current) {
                bail!("organization hierarchy contains a cycle at `{current}`");
            }
            chain.push(current);
            let record = by_id[current];
            current = parent_id(record);
        }
        complete.extend(chain);
    }

    for record in threads {
        if record.role == NodeRole::Worker && record.can_spawn {
            bail!("worker node `{}` cannot have spawn permission", record.id);
        }
        let parent = parent_id(record);
        if parent == ROOT_ID {
            continue;
        }
        let parent_record = by_id[parent];
        if parent_record.role == NodeRole::Worker {
            bail!("worker node `{parent}` cannot spawn child `{}`", record.id);
        }
        if !parent_record.can_spawn {
            bail!(
                "node `{parent}` does not have permission to spawn child `{}`",
                record.id
            );
        }
    }
    Ok(by_id)
}

fn compare_ids(left: &str, right: &str) -> Ordering {
    let left_number = left.strip_prefix("t-").and_then(|n| n.parse::<u64>().ok());
    let right_number = right.strip_prefix("t-").and_then(|n| n.parse::<u64>().ok());
    match (left_number, right_number) {
        (Some(a), Some(b)) => a.cmp(&b).then_with(|| left.cmp(right)),
        _ => left.cmp(right),
    }
}

pub fn parent_id(record: &Thread) -> &str {
    if record.parent_id.is_empty() {
        ROOT_ID
    } else {
        &record.parent_id
    }
}

pub fn prepare_node(project: &Project, request: &NodeRequest) -> Result<(bool, AgentProfile)> {
    let records = thread::list(project);
    validate_tree(&records)?;
    prepare_node_from(project, &records, request)
}

/// When the caller is a known project pane, keep its child creation beneath
/// itself. Calls from ordinary terminal sessions remain manually usable.
pub fn authorize_creator(
    project: &Project,
    caller_pane: &str,
    caller_socket: &str,
    requested_parent: &str,
) -> Result<()> {
    let Some(creator_id) = creator_parent(project, caller_pane, caller_socket)? else {
        return Ok(());
    };
    let requested_parent = if requested_parent.is_empty() {
        ROOT_ID
    } else {
        requested_parent
    };
    if requested_parent != creator_id {
        bail!(
            "coordinator `{creator_id}` may create children only beneath itself, not `{requested_parent}`"
        );
    }
    Ok(())
}

/// Applies the shared pane hierarchy check and returns the parent a recognized
/// coordinator is authorized to use. `None` means this is not a known project
/// pane, preserving manually selected parents for terminal callers.
pub fn creator_parent(
    project: &Project,
    caller_pane: &str,
    caller_socket: &str,
) -> Result<Option<String>> {
    if caller_pane.is_empty() || caller_socket.is_empty() {
        return Ok(None);
    }
    let coordinator = project.coordinator();
    let is_root_coordinator = coordinator
        .as_ref()
        .is_some_and(|record| record.socket == caller_socket && record.pane_id == caller_pane);
    let records = thread::list(project);
    validate_tree(&records)?;
    let known_node = coordinator
        .as_ref()
        .is_some_and(|coordinator| coordinator.socket == caller_socket)
        .then(|| {
            records
                .into_iter()
                .find(|record| record.machine.is_empty() && record.pane_id == caller_pane)
        })
        .flatten();
    let (creator_id, role, can_spawn) = if is_root_coordinator {
        (ROOT_ID.to_string(), NodeRole::Coordinator, true)
    } else if let Some(record) = known_node {
        (record.id, record.role, record.can_spawn)
    } else {
        return Ok(None);
    };
    if role != NodeRole::Coordinator {
        bail!("worker nodes cannot create children");
    }
    if !can_spawn {
        bail!("node `{creator_id}` does not have permission to create children");
    }
    Ok(Some(creator_id))
}

/// Remote workers can use the existing remote Herdr and SSH flow. Their
/// coordinator brief cannot safely launch descendants from the home machine.
pub fn validate_machine_spawn(role: NodeRole, can_spawn: bool, machine: &str) -> Result<()> {
    if !machine.is_empty() && role == NodeRole::Coordinator && can_spawn {
        bail!(
            "remote coordinators with child-spawn permission are unsupported; remote workers remain supported, and remote recursive coordinators require a future bridge"
        );
    }
    Ok(())
}

fn prepare_node_from(
    project: &Project,
    records: &[Thread],
    request: &NodeRequest,
) -> Result<(bool, AgentProfile)> {
    let parent_id = if request.parent_id.is_empty() {
        ROOT_ID
    } else {
        &request.parent_id
    };
    validate_parent(records, parent_id)?;
    let can_spawn = request
        .can_spawn
        .unwrap_or(request.role == NodeRole::Coordinator);
    if request.role == NodeRole::Worker && can_spawn {
        bail!("worker nodes cannot spawn children; omit `--can-spawn` or pass `--can-spawn=false`");
    }

    let mut profile = if parent_id == ROOT_ID {
        root_profile(project, request.role)?
    } else {
        profile_for_record(project, records, parent_id)?
    };
    if request
        .profile
        .harness
        .as_ref()
        .is_some_and(|harness| harness != &profile.harness)
    {
        profile.raw_agent_args.clear();
    }
    profile.apply(&request.profile);
    profile.argv(&[])?;
    Ok((can_spawn, profile))
}

fn validate_parent(records: &[Thread], parent_id: &str) -> Result<()> {
    if parent_id == ROOT_ID {
        return Ok(());
    }
    thread::validate_id(parent_id)?;
    let parent = records
        .iter()
        .find(|record| record.id == parent_id)
        .with_context(|| format!("parent node `{parent_id}` does not exist"))?;
    if parent.role == NodeRole::Worker {
        bail!("worker node `{parent_id}` cannot spawn children");
    }
    if !parent.can_spawn {
        bail!("node `{parent_id}` does not have permission to spawn children");
    }
    Ok(())
}

fn root_profile(project: &Project, role: NodeRole) -> Result<AgentProfile> {
    let (settings, _) = project.read_project_md()?;
    Ok(AgentProfile {
        harness: match role {
            NodeRole::Worker => settings.thread_agent,
            NodeRole::Coordinator => settings.coordinator_agent,
        },
        ..AgentProfile::default()
    })
}

fn profile_for_record(
    project: &Project,
    records: &[Thread],
    target_id: &str,
) -> Result<AgentProfile> {
    let record = records
        .iter()
        .find(|record| record.id == target_id)
        .with_context(|| format!("node `{target_id}` does not exist"))?;
    let mut profile = root_profile(project, record.role)?;
    profile.apply(&ProfileOverrides {
        harness: (!record.agent.is_empty()).then(|| record.agent.clone()),
        model: (!record.model.is_empty()).then(|| record.model.clone()),
        reasoning_effort: (!record.reasoning_effort.is_empty())
            .then(|| record.reasoning_effort.clone()),
        permission_profile: (!record.permission_profile.is_empty())
            .then(|| record.permission_profile.clone()),
        raw_agent_args: record.raw_agent_args.clone(),
    });
    Ok(profile)
}

pub fn create_node(project: &Project, args: &CreateNode) -> Result<Thread> {
    if args.title.trim().is_empty() {
        bail!("--title may not be empty");
    }
    if args.task.trim().is_empty() {
        bail!("the task is empty");
    }
    let _lock = project.lock()?;
    let records = thread::list(project);
    validate_tree(&records)?;
    let (can_spawn, profile) = prepare_node_from(project, &records, &args.request)?;
    validate_machine_spawn(args.request.role, can_spawn, &args.machine)?;
    let record = thread::allocate_locked(project, |record| {
        record.title = args.title.trim().to_string();
        record.status = Status::Starting;
        record.kind = args.kind;
        record.repo = args.repo.clone();
        record.machine = args.machine.clone();
        record.base = args.base.clone();
        record.parent_id = if args.request.parent_id.is_empty() {
            ROOT_ID.into()
        } else {
            args.request.parent_id.clone()
        };
        record.role = args.request.role;
        record.can_spawn = can_spawn;
        record.agent = profile.harness.clone();
        record.model = profile.model.clone();
        record.reasoning_effort = profile.reasoning_effort.clone();
        record.permission_profile = profile.permission_profile.clone();
        record.raw_agent_args = profile.raw_agent_args.clone();
    })?;

    let scope = node_scope_dir(project, &record.id);
    let task_path = thread::task_path(project, &record.id);
    if task_path.exists() {
        thread::remove_record_locked(project, &record.id);
        bail!("task file {} already exists", task_path.display());
    }
    let nodes_created = match create_node_scope(&scope) {
        Ok(created) => created,
        Err(error) => {
            thread::remove_record_locked(project, &record.id);
            return Err(error)
                .with_context(|| format!("could not finish creating node `{}`", record.id));
        }
    };
    if let Err(error) = project::write_atomic(&task_path, args.task.as_bytes()) {
        let _ = std::fs::remove_dir_all(&scope);
        if nodes_created && let Some(nodes) = scope.parent() {
            let _ = std::fs::remove_dir(nodes);
        }
        thread::remove_record_locked(project, &record.id);
        return Err(error)
            .with_context(|| format!("could not finish creating node `{}`", record.id));
    }
    Ok(record)
}

pub fn node_scope_dir(project: &Project, id: &str) -> PathBuf {
    project.dir().join("nodes").join(id)
}

fn create_node_scope(scope: &Path) -> Result<bool> {
    let nodes = scope.parent().context("node scope has no parent")?;
    let nodes_created = ensure_directory(nodes)?;
    if let Err(error) = std::fs::create_dir(scope) {
        if nodes_created {
            let _ = std::fs::remove_dir(nodes);
        }
        return Err(error)
            .with_context(|| format!("could not create node scope {}", scope.display()));
    }
    let result = (|| -> Result<()> {
        std::fs::create_dir(scope.join("memory")).with_context(|| {
            format!(
                "could not create node memory directory in {}",
                scope.display()
            )
        })?;
        project::write_atomic(
            &scope.join("INSTRUCTIONS.md"),
            b"# Node instructions\n\nStanding instructions for this node and its descendants.\n",
        )?;
        project::write_atomic(&scope.join("MEMORY.md"), b"# Node memory\n")?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_dir_all(scope);
        if nodes_created && let Some(nodes) = scope.parent() {
            let _ = std::fs::remove_dir(nodes);
        }
    }
    result.map(|()| nodes_created)
}

fn ensure_directory(path: &Path) -> Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_dir() => Ok(false),
        Ok(_) => bail!("{} must be a regular directory", path.display()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => std::fs::create_dir(path)
            .with_context(|| format!("could not create {}", path.display()))
            .map(|()| true),
        Err(error) => Err(error).with_context(|| format!("could not inspect {}", path.display())),
    }
}

pub fn scoped_context(project: &Project, target: &Thread) -> Result<ScopedContext> {
    let records = thread::list(project);
    let by_id = validate_tree(&records)?;
    let mut chain = Vec::new();
    let mut current = target.id.as_str();
    while current != ROOT_ID {
        let record = by_id
            .get(current)
            .with_context(|| format!("node `{current}` is not in the organization tree"))?;
        chain.push(*record);
        current = parent_id(record);
    }
    chain.reverse();

    let (_, project_instructions) = project.read_project_md()?;
    let mut instructions = format!("## Project root\n\n{}", project_instructions.trim());
    let mut budget = ScopedMemoryBudget::new();
    let mut memory_index = String::from("## Project root\n\n");
    if let Some(root_memory) =
        read_scoped_memory_file(&project.dir().join("MEMORY.md"), "MEMORY.md", &mut budget)?
    {
        memory_index.push_str(root_memory.trim());
    }
    let mut memory_files = Vec::new();
    read_memory_files(
        &project.dir().join("memory"),
        "memory",
        &mut budget,
        &mut memory_files,
    )?;

    let nodes_dir = project.dir().join("nodes");
    let nodes_dir_exists = optional_real_directory(&nodes_dir)?;
    for record in chain {
        let scope = node_scope_dir(project, &record.id);
        if !nodes_dir_exists || !optional_real_directory(&scope)? {
            continue;
        }
        let node_instructions = read_optional_text(&scope.join("INSTRUCTIONS.md"), true)?;
        if !node_instructions.trim().is_empty() {
            instructions.push_str(&format!(
                "\n\n## Node {} instructions\n\n{}",
                record.id,
                node_instructions.trim()
            ));
        }
        let memory_path = format!("nodes/{}/MEMORY.md", record.id);
        if let Some(node_memory) =
            read_scoped_memory_file(&scope.join("MEMORY.md"), &memory_path, &mut budget)?
            && !node_memory.trim().is_empty()
        {
            memory_index.push_str(&format!(
                "\n\n## Node {} memory\n\n{}",
                record.id,
                node_memory.trim()
            ));
        }
        read_memory_files(
            &scope.join("memory"),
            &format!("nodes/{}/memory", record.id),
            &mut budget,
            &mut memory_files,
        )?;
    }
    let omissions = budget.omission_markdown();
    if !omissions.is_empty() {
        memory_index.push_str(&omissions);
    }
    Ok(ScopedContext {
        instructions,
        memory_index,
        memory_files,
    })
}

fn optional_real_directory(path: &Path) -> Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_dir() => Ok(true),
        Ok(_) => bail!("{} must be a regular directory", path.display()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error).with_context(|| format!("could not inspect {}", path.display())),
    }
}

fn read_optional_text(path: &Path, refuse_symlink: bool) -> Result<String> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() => {
            read_regular_file(path).with_context(|| format!("could not read {}", path.display()))
        }
        Ok(_) if refuse_symlink => bail!("{} must be a regular file", path.display()),
        Ok(_) => Ok(String::new()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(error) => Err(error).with_context(|| format!("could not inspect {}", path.display())),
    }
}

fn read_regular_file(path: &Path) -> Result<String> {
    let mut file = open_regular_file_no_follow(path)?;
    let mut contents = String::new();
    file.read_to_string(&mut contents)?;
    Ok(contents)
}

fn open_regular_file_no_follow(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let file = options.open(path)?;
    if !file.metadata()?.file_type().is_file() {
        bail!("{} must be a regular file", path.display());
    }
    Ok(file)
}

#[derive(Debug)]
struct ScopedMemoryBudget {
    files_considered: usize,
    bytes_read: usize,
    omissions: Vec<String>,
    omitted_count: usize,
}

impl ScopedMemoryBudget {
    fn new() -> Self {
        Self {
            files_considered: 0,
            bytes_read: 0,
            omissions: Vec::new(),
            omitted_count: 0,
        }
    }

    fn has_file_slot(&self) -> bool {
        self.files_considered < MAX_SCOPED_MEMORY_FILES
    }

    fn take_file_slot(&mut self) -> bool {
        if !self.has_file_slot() {
            return false;
        }
        self.files_considered += 1;
        true
    }

    fn remaining_bytes(&self) -> usize {
        MAX_SCOPED_MEMORY_TOTAL_BYTES.saturating_sub(self.bytes_read)
    }

    fn record_read(&mut self, bytes: usize) {
        self.bytes_read = self.bytes_read.saturating_add(bytes);
    }

    fn omit(&mut self, detail: String) {
        self.omitted_count = self.omitted_count.saturating_add(1);
        if self.omissions.len() < MAX_MEMORY_OMISSION_DETAILS {
            self.omissions.push(detail);
        }
    }

    fn omission_markdown(&self) -> String {
        if self.omitted_count == 0 {
            return String::new();
        }
        let mut text = String::from("\n\n## Scoped memory omitted\n\n");
        for detail in &self.omissions {
            text.push_str("- ");
            text.push_str(detail);
            text.push('\n');
        }
        let unlisted = self.omitted_count.saturating_sub(self.omissions.len());
        if unlisted > 0 {
            text.push_str(&format!(
                "- {unlisted} additional omitted items are not listed.\n"
            ));
        }
        text
    }
}

fn scoped_memory_label(prefix: &str, name: &str) -> String {
    let clean_name = name
        .chars()
        .filter(|character| !character.is_control())
        .collect::<String>()
        .replace('`', "'");
    format!("{prefix}/{clean_name}")
}

fn read_scoped_memory_file(
    path: &Path,
    display_path: &str,
    budget: &mut ScopedMemoryBudget,
) -> Result<Option<String>> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| format!("could not inspect {}", path.display()));
        }
    };
    if !metadata.file_type().is_file() {
        budget.omit(format!("{display_path}: refused non-regular file"));
        return Ok(None);
    }
    if !budget.take_file_slot() {
        budget.omit(format!("{display_path}: scoped file-count limit reached"));
        return Ok(None);
    }
    if metadata.len() > MAX_SCOPED_MEMORY_FILE_BYTES as u64 {
        budget.omit(format!(
            "{display_path}: exceeds the {MAX_SCOPED_MEMORY_FILE_BYTES}-byte per-file limit"
        ));
        return Ok(None);
    }
    if metadata.len() > budget.remaining_bytes() as u64 {
        budget.omit(format!(
            "{display_path}: exceeds the {MAX_SCOPED_MEMORY_TOTAL_BYTES}-byte aggregate limit"
        ));
        return Ok(None);
    }

    let file = open_regular_file_no_follow(path).with_context(|| {
        format!(
            "could not safely open scoped memory file {}",
            path.display()
        )
    })?;
    let opened_len = file.metadata()?.len();
    if opened_len > MAX_SCOPED_MEMORY_FILE_BYTES as u64 {
        budget.omit(format!(
            "{display_path}: exceeds the {MAX_SCOPED_MEMORY_FILE_BYTES}-byte per-file limit"
        ));
        return Ok(None);
    }
    if opened_len > budget.remaining_bytes() as u64 {
        budget.omit(format!(
            "{display_path}: exceeds the {MAX_SCOPED_MEMORY_TOTAL_BYTES}-byte aggregate limit"
        ));
        return Ok(None);
    }

    let mut limited = file.take(opened_len);
    let mut bytes = Vec::with_capacity(opened_len as usize);
    limited.read_to_end(&mut bytes)?;
    budget.record_read(bytes.len());
    let final_len = limited.get_ref().metadata()?.len();
    if bytes.len() as u64 != opened_len || final_len > bytes.len() as u64 {
        budget.omit(format!("{display_path}: changed while it was being read"));
        return Ok(None);
    }
    String::from_utf8(bytes)
        .map(Some)
        .with_context(|| format!("scoped memory file {} is not UTF-8", path.display()))
}

fn read_memory_files(
    memory_dir: &Path,
    prefix: &str,
    budget: &mut ScopedMemoryBudget,
    output: &mut Vec<(String, String)>,
) -> Result<()> {
    match std::fs::symlink_metadata(memory_dir) {
        Ok(metadata) if metadata.file_type().is_dir() => {}
        Ok(_) => bail!("{} must be a regular directory", memory_dir.display()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("could not inspect {}", memory_dir.display()));
        }
    }
    if !budget.has_file_slot() {
        budget.omit(format!(
            "{prefix}/*.md: not read because the scoped file-count limit was reached"
        ));
        return Ok(());
    }

    let mut names = BTreeSet::new();
    let mut discovered = 0usize;
    for entry in std::fs::read_dir(memory_dir)
        .with_context(|| format!("could not list {}", memory_dir.display()))?
    {
        let entry = entry?;
        let Some(name) = entry.file_name().into_string().ok() else {
            continue;
        };
        if !name.ends_with(".md") || name.starts_with('.') {
            continue;
        }
        discovered = discovered.saturating_add(1);
        if names.len() < MAX_MEMORY_NAMES_PER_DIRECTORY {
            names.insert(name);
        } else if names
            .last()
            .is_some_and(|last| name.as_str() < last.as_str())
        {
            names.pop_last();
            names.insert(name);
        }
    }

    for name in names {
        let display_path = scoped_memory_label(prefix, &name);
        if let Some(contents) =
            read_scoped_memory_file(&memory_dir.join(&name), &display_path, budget)?
        {
            output.push((display_path, contents));
        }
    }
    if discovered > MAX_MEMORY_NAMES_PER_DIRECTORY {
        budget.omit(format!(
            "{prefix}/*.md: {} later sorted files were not examined",
            discovered - MAX_MEMORY_NAMES_PER_DIRECTORY
        ));
    }
    Ok(())
}

pub fn node_protocol(record: &Thread, command_prefix: &str, slug: &str) -> String {
    let can_spawn = if record.can_spawn { "yes" } else { "no" };
    let mut text = format!(
        "Node: `{}`\nParent: `{}`\nRole: `{}`\nCan spawn children: `{can_spawn}`\n",
        record.id,
        parent_id(record),
        record.role.as_str()
    );
    if record.role == NodeRole::Coordinator && record.can_spawn && record.is_remote() {
        text.push_str("\nRemote recursive coordinators are unsupported until a remote CLI bridge is available. Remote workers remain supported.\n");
    } else if record.role == NodeRole::Coordinator && record.can_spawn {
        text.push_str(&format!(
            "\n# Creating child nodes\n\nUse this CLI protocol for every child. Always set `--parent` to your own node id (`{}`), so instructions and memory follow the ancestor chain. A child coordinator may create its own descendants; a worker cannot spawn.\n\n```sh\n{command_prefix} node start {slug} --parent {} --role worker --title \"Short task\" --task-file - <<'TASK'\nDescribe the task, repository and acceptance criteria.\nTASK\n```\n\nChoose `--role coordinator` for a child that must plan and delegate. Optional profile flags are `--harness`, `--model`, `--reasoning-effort`, `--permission-profile`, and repeatable `--raw-agent-arg`. Omitted profile fields inherit the parent profile at creation.\n",
            record.id, record.id
        ));
    } else if record.role == NodeRole::Coordinator {
        text.push_str("\nThis is a leaf coordinator and cannot create child nodes because `can_spawn` is false.\n");
    } else {
        text.push_str("\nWorkers do not create child nodes. The CLI rejects attempts to use a worker as a parent.\n");
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scenarios::World;

    fn node(id: &str, parent_id: &str, role: NodeRole, can_spawn: bool) -> Thread {
        Thread {
            id: id.into(),
            parent_id: parent_id.into(),
            role,
            can_spawn,
            ..Thread::default()
        }
    }

    fn request(parent_id: &str, role: NodeRole) -> NodeRequest {
        NodeRequest {
            parent_id: parent_id.into(),
            role,
            ..NodeRequest::default()
        }
    }

    #[test]
    fn traversal_is_recursive_and_stable_at_sixteen_levels() {
        let world = World::new();
        let project = world.project("demo", "a.sock");
        let mut parent = ROOT_ID.to_string();
        for _ in 0..16 {
            let created = create_node(
                &project,
                &CreateNode {
                    request: request(&parent, NodeRole::Coordinator),
                    title: format!("Level {parent}"),
                    kind: Kind::Tab,
                    repo: String::new(),
                    machine: String::new(),
                    base: String::new(),
                    task: "continue".into(),
                },
            )
            .unwrap();
            parent = created.id;
        }
        let first = tree(&project).unwrap();
        let second = tree(&project).unwrap();
        assert_eq!(first.len(), 16);
        assert_eq!(first, second);
        assert_eq!(first[15].depth, 16);
        assert_eq!(first[15].tree_order, 16);
        let reloaded = Project::load(&world.root, "demo").unwrap();
        assert_eq!(tree(&reloaded).unwrap(), first);
    }

    #[test]
    fn scrambled_branching_tree_keeps_sibling_order_prefixes_and_last_flags() {
        let entries = tree_from(&[
            node("t-0007", "t-0002", NodeRole::Worker, false),
            node("t-0002", ROOT_ID, NodeRole::Coordinator, true),
            node("t-0004", "t-0001", NodeRole::Worker, false),
            node("t-0003", ROOT_ID, NodeRole::Worker, false),
            node("t-0006", "t-0002", NodeRole::Worker, false),
            node("t-0001", ROOT_ID, NodeRole::Coordinator, true),
            node("t-0005", "t-0001", NodeRole::Worker, false),
        ])
        .unwrap();

        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.thread.id.as_str())
                .collect::<Vec<_>>(),
            [
                "t-0001", "t-0004", "t-0005", "t-0002", "t-0006", "t-0007", "t-0003"
            ]
        );
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.prefix.as_str())
                .collect::<Vec<_>>(),
            ["├─ ", "│  ├─ ", "│  └─ ", "├─ ", "│  ├─ ", "│  └─ ", "└─ "]
        );
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.is_last)
                .collect::<Vec<_>>(),
            [false, false, true, false, false, true, true]
        );
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.tree_order)
                .collect::<Vec<_>>(),
            [1, 2, 3, 4, 5, 6, 7]
        );
    }

    #[test]
    fn rejects_missing_parent_self_parent_cycles_and_worker_spawn_without_mutation() {
        for invalid in [
            vec![node("t-0001", "t-0002", NodeRole::Coordinator, true)],
            vec![node("t-0001", "t-0001", NodeRole::Coordinator, true)],
            vec![node("t-0001", ROOT_ID, NodeRole::Worker, true)],
            vec![
                node("t-0001", "t-0002", NodeRole::Coordinator, true),
                node("t-0002", "t-0001", NodeRole::Coordinator, true),
            ],
        ] {
            let error = tree_from(&invalid).unwrap_err().to_string();
            assert!(
                error.contains("missing parent")
                    || error.contains("own parent")
                    || error.contains("cycle")
                    || error.contains("spawn permission"),
                "{error}"
            );
        }

        let world = World::new();
        let project = world.project("demo", "a.sock");
        let worker = create_node(
            &project,
            &CreateNode {
                request: request(ROOT_ID, NodeRole::Worker),
                title: "Worker".into(),
                kind: Kind::Tab,
                repo: String::new(),
                machine: String::new(),
                base: String::new(),
                task: "task".into(),
            },
        )
        .unwrap();
        let before = thread::list(&project);
        let error = create_node(
            &project,
            &CreateNode {
                request: request(&worker.id, NodeRole::Worker),
                title: "Rejected".into(),
                kind: Kind::Tab,
                repo: String::new(),
                machine: String::new(),
                base: String::new(),
                task: "must not persist".into(),
            },
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("cannot spawn children"), "{error}");
        assert_eq!(thread::list(&project), before);
        assert!(!thread::task_path(&project, "t-0002").exists());
        assert!(!node_scope_dir(&project, "t-0002").exists());
    }

    #[test]
    fn remote_recursive_coordinator_creation_is_atomic_and_remote_workers_remain_allowed() {
        let world = World::new();
        let project = world.project("demo", "a.sock");
        let remote_coordinator = CreateNode {
            request: request(ROOT_ID, NodeRole::Coordinator),
            title: "Remote coordinator".into(),
            kind: Kind::Worktree,
            repo: "/srv/repo".into(),
            machine: "build-server".into(),
            base: String::new(),
            task: "Coordinate remote work".into(),
        };
        let error = create_node(&project, &remote_coordinator)
            .unwrap_err()
            .to_string();
        assert!(error.contains("remote recursive coordinators require a future bridge"));
        assert!(thread::list(&project).is_empty());
        assert!(!project.dir().join("nodes").exists());
        assert!(!thread::task_path(&project, "t-0001").exists());

        let remote_worker = CreateNode {
            request: request(ROOT_ID, NodeRole::Worker),
            title: "Remote worker".into(),
            kind: Kind::Worktree,
            repo: "/srv/repo".into(),
            machine: "build-server".into(),
            base: String::new(),
            task: "Implement a scoped task".into(),
        };
        let worker = create_node(&project, &remote_worker).unwrap();
        assert_eq!(worker.role, NodeRole::Worker);
        assert_eq!(worker.machine, "build-server");
    }

    #[test]
    fn known_worker_cannot_spawn_and_coordinator_must_use_itself_as_parent() {
        let world = World::new();
        let project = world.project("demo", "a.sock");
        let socket = project.coordinator().unwrap().socket;
        let worker = create_node(
            &project,
            &CreateNode {
                request: request(ROOT_ID, NodeRole::Worker),
                title: "Worker".into(),
                kind: Kind::Tab,
                repo: String::new(),
                machine: String::new(),
                base: String::new(),
                task: "work".into(),
            },
        )
        .unwrap();
        thread::update(&project, &worker.id, |record| {
            record.pane_id = "worker-pane".into()
        })
        .unwrap();
        assert!(
            authorize_creator(&project, "worker-pane", &socket, ROOT_ID)
                .unwrap_err()
                .to_string()
                .contains("worker nodes cannot")
        );

        let coordinator = create_node(
            &project,
            &CreateNode {
                request: request(ROOT_ID, NodeRole::Coordinator),
                title: "Coordinator".into(),
                kind: Kind::Tab,
                repo: String::new(),
                machine: String::new(),
                base: String::new(),
                task: "coordinate".into(),
            },
        )
        .unwrap();
        thread::update(&project, &coordinator.id, |record| {
            record.pane_id = "coordinator-pane".into()
        })
        .unwrap();
        authorize_creator(&project, "coordinator-pane", &socket, &coordinator.id).unwrap();
        assert!(
            authorize_creator(&project, "coordinator-pane", &socket, ROOT_ID)
                .unwrap_err()
                .to_string()
                .contains("only beneath itself")
        );
    }

    #[test]
    fn coordinator_without_spawn_permission_cannot_create_a_child() {
        let world = World::new();
        let project = world.project("demo", "a.sock");
        let mut leaf_request = request(ROOT_ID, NodeRole::Coordinator);
        leaf_request.can_spawn = Some(false);
        let leaf = create_node(
            &project,
            &CreateNode {
                request: leaf_request,
                title: "Leaf coordinator".into(),
                kind: Kind::Tab,
                repo: String::new(),
                machine: String::new(),
                base: String::new(),
                task: "coordinate without children".into(),
            },
        )
        .unwrap();
        let before = thread::list(&project);
        let error = create_node(
            &project,
            &CreateNode {
                request: request(&leaf.id, NodeRole::Worker),
                title: "Rejected child".into(),
                kind: Kind::Tab,
                repo: String::new(),
                machine: String::new(),
                base: String::new(),
                task: "must not persist".into(),
            },
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("does not have permission"), "{error}");
        assert_eq!(thread::list(&project), before);
        assert!(!thread::task_path(&project, "t-0002").exists());
        assert!(!node_scope_dir(&project, "t-0002").exists());
    }

    #[test]
    fn invalid_existing_trees_do_not_mutate_records_or_create_nodes() {
        let invalid_trees = [
            vec![node("t-0001", "t-9999", NodeRole::Coordinator, true)],
            vec![node("t-0001", "t-0001", NodeRole::Coordinator, true)],
            vec![
                node("t-0001", "t-0002", NodeRole::Coordinator, true),
                node("t-0002", "t-0001", NodeRole::Coordinator, true),
            ],
        ];
        for invalid in invalid_trees {
            let world = World::new();
            let project = world.project("demo", "a.sock");
            let mut before = HashMap::new();
            for record in invalid {
                let path = thread::record_path(&project, &record.id);
                std::fs::write(&path, toml::to_string(&record).unwrap()).unwrap();
                before.insert(record.id.clone(), std::fs::read(path).unwrap());
            }
            let error = create_node(
                &project,
                &CreateNode {
                    request: request(ROOT_ID, NodeRole::Worker),
                    title: "Must not persist".into(),
                    kind: Kind::Tab,
                    repo: String::new(),
                    machine: String::new(),
                    base: String::new(),
                    task: "rejected".into(),
                },
            )
            .unwrap_err()
            .to_string();
            assert!(
                error.contains("missing parent")
                    || error.contains("own parent")
                    || error.contains("cycle"),
                "{error}"
            );
            for (id, bytes) in before {
                assert_eq!(
                    std::fs::read(thread::record_path(&project, &id)).unwrap(),
                    bytes
                );
            }
            assert!(!thread::task_path(&project, "t-0003").exists());
            assert!(!node_scope_dir(&project, "t-0003").exists());
        }
    }

    #[test]
    fn scoped_context_contains_only_root_ancestors_and_target_in_order() {
        let world = World::new();
        let project = world.project("demo", "a.sock");
        let other = world.project("other", "b.sock");
        std::fs::write(
            project.dir().join("PROJECT.md"),
            "+++\nname = \"demo\"\n+++\nROOT-SENTINEL\n",
        )
        .unwrap();
        std::fs::write(project.dir().join("MEMORY.md"), "ROOT-MEMORY\n").unwrap();
        let root_memory = project.dir().join("memory");
        std::fs::create_dir_all(&root_memory).unwrap();
        std::fs::write(root_memory.join("root.md"), "ROOT-FILE\n").unwrap();

        let ancestor = create_node(
            &project,
            &CreateNode {
                request: request(ROOT_ID, NodeRole::Coordinator),
                title: "Ancestor".into(),
                kind: Kind::Tab,
                repo: String::new(),
                machine: String::new(),
                base: String::new(),
                task: "ancestor task".into(),
            },
        )
        .unwrap();
        let sibling = create_node(
            &project,
            &CreateNode {
                request: request(ROOT_ID, NodeRole::Coordinator),
                title: "Sibling".into(),
                kind: Kind::Tab,
                repo: String::new(),
                machine: String::new(),
                base: String::new(),
                task: "sibling task".into(),
            },
        )
        .unwrap();
        let target = create_node(
            &project,
            &CreateNode {
                request: request(&ancestor.id, NodeRole::Worker),
                title: "Target".into(),
                kind: Kind::Tab,
                repo: String::new(),
                machine: String::new(),
                base: String::new(),
                task: "target task".into(),
            },
        )
        .unwrap();
        let descendant = create_node(
            &project,
            &CreateNode {
                request: request(&ancestor.id, NodeRole::Coordinator),
                title: "Descendant".into(),
                kind: Kind::Tab,
                repo: String::new(),
                machine: String::new(),
                base: String::new(),
                task: "descendant task".into(),
            },
        )
        .unwrap();
        write_scope_sentinel(
            &project,
            &ancestor.id,
            "ANCESTOR-SENTINEL",
            "ANCESTOR-MEMORY",
        );
        write_scope_sentinel(&project, &target.id, "TARGET-SENTINEL", "TARGET-MEMORY");
        write_scope_sentinel(&project, &sibling.id, "SIBLING-SENTINEL", "SIBLING-MEMORY");
        write_scope_sentinel(
            &project,
            &descendant.id,
            "DESCENDANT-SENTINEL",
            "DESCENDANT-MEMORY",
        );
        std::fs::write(
            other.dir().join("PROJECT.md"),
            "+++\nname = \"other\"\n+++\nOTHER-PROJECT-SENTINEL\n",
        )
        .unwrap();

        let scoped = scoped_context(&project, &target).unwrap();
        let brief = format!(
            "{}\n{}\n{:?}",
            scoped.instructions, scoped.memory_index, scoped.memory_files
        );
        let positions = ["ROOT-SENTINEL", "ANCESTOR-SENTINEL", "TARGET-SENTINEL"]
            .map(|needle| brief.find(needle).unwrap());
        assert!(
            positions.windows(2).all(|pair| pair[0] < pair[1]),
            "{brief}"
        );
        for excluded in [
            "SIBLING-SENTINEL",
            "DESCENDANT-SENTINEL",
            "OTHER-PROJECT-SENTINEL",
            "SIBLING-MEMORY",
            "DESCENDANT-MEMORY",
        ] {
            assert!(!brief.contains(excluded), "leaked {excluded} in {brief}");
        }
        assert!(brief.contains("ROOT-MEMORY"));
        assert!(brief.contains("ANCESTOR-MEMORY"));
        assert!(brief.contains("TARGET-MEMORY"));
        assert_eq!(
            scoped
                .memory_files
                .iter()
                .map(|(path, _)| path.clone())
                .collect::<Vec<_>>(),
            vec![
                "memory/root.md".to_string(),
                format!("nodes/{}/memory/scope.md", ancestor.id),
                format!("nodes/{}/memory/scope.md", target.id),
            ],
        );
    }

    #[test]
    fn scoped_memory_enforces_per_file_and_aggregate_limits_deterministically() {
        let world = World::new();
        let project = world.project("demo", "a.sock");
        let target = create_node(
            &project,
            &CreateNode {
                request: request(ROOT_ID, NodeRole::Worker),
                title: "Target".into(),
                kind: Kind::Tab,
                repo: String::new(),
                machine: String::new(),
                base: String::new(),
                task: "task".into(),
            },
        )
        .unwrap();
        std::fs::remove_file(project.dir().join("MEMORY.md")).unwrap();
        std::fs::remove_file(node_scope_dir(&project, &target.id).join("MEMORY.md")).unwrap();

        std::fs::write(
            project.dir().join("MEMORY.md"),
            format!(
                "MEMORY-INDEX-OVERSIZED-SENTINEL{}",
                "x".repeat(MAX_SCOPED_MEMORY_FILE_BYTES)
            ),
        )
        .unwrap();
        let memory = project.dir().join("memory");
        std::fs::write(
            memory.join("00-too-big.md"),
            format!(
                "PER-FILE-OVERSIZED-SENTINEL{}",
                "x".repeat(MAX_SCOPED_MEMORY_FILE_BYTES)
            ),
        )
        .unwrap();
        for number in 1..=4 {
            std::fs::write(
                memory.join(format!("0{number}-block.md")),
                "b".repeat(MAX_SCOPED_MEMORY_FILE_BYTES),
            )
            .unwrap();
        }
        std::fs::write(memory.join("05-small.md"), "AFTER-AGGREGATE-SENTINEL").unwrap();

        let scoped = scoped_context(&project, &target).unwrap();
        let read_bytes = scoped
            .memory_files
            .iter()
            .map(|(_, contents)| contents.len())
            .sum::<usize>();
        assert!(read_bytes <= MAX_SCOPED_MEMORY_TOTAL_BYTES);
        assert_eq!(scoped.memory_files.len(), 4);
        assert_eq!(scoped.memory_files[0].0, "memory/01-block.md");
        assert_eq!(scoped.memory_files[1].0, "memory/02-block.md");
        assert_eq!(scoped.memory_files[2].0, "memory/03-block.md");
        assert_eq!(scoped.memory_files[3].0, "memory/05-small.md");
        assert!(
            scoped
                .memory_index
                .contains("MEMORY.md: exceeds the 8192-byte per-file limit")
        );
        assert!(
            scoped
                .memory_index
                .contains("memory/00-too-big.md: exceeds the 8192-byte per-file limit")
        );
        assert!(
            scoped
                .memory_index
                .contains("memory/04-block.md: exceeds the 32000-byte aggregate limit")
        );
        assert!(
            !scoped
                .memory_index
                .contains("MEMORY-INDEX-OVERSIZED-SENTINEL")
        );
        assert!(
            scoped
                .memory_files
                .iter()
                .all(|(_, contents)| !contents.contains("PER-FILE-OVERSIZED-SENTINEL"))
        );
    }

    #[test]
    fn scoped_memory_caps_file_count_and_excludes_sibling_sentinels() {
        let world = World::new();
        let project = world.project("demo", "a.sock");
        let sibling = create_node(
            &project,
            &CreateNode {
                request: request(ROOT_ID, NodeRole::Worker),
                title: "Sibling".into(),
                kind: Kind::Tab,
                repo: String::new(),
                machine: String::new(),
                base: String::new(),
                task: "sibling".into(),
            },
        )
        .unwrap();
        let target = create_node(
            &project,
            &CreateNode {
                request: request(ROOT_ID, NodeRole::Worker),
                title: "Target".into(),
                kind: Kind::Tab,
                repo: String::new(),
                machine: String::new(),
                base: String::new(),
                task: "target".into(),
            },
        )
        .unwrap();
        std::fs::remove_file(project.dir().join("MEMORY.md")).unwrap();
        std::fs::remove_file(node_scope_dir(&project, &sibling.id).join("MEMORY.md")).unwrap();
        std::fs::remove_file(node_scope_dir(&project, &target.id).join("MEMORY.md")).unwrap();
        std::fs::write(
            node_scope_dir(&project, &sibling.id).join("memory/sentinel.md"),
            "SIBLING-MEMORY-SENTINEL",
        )
        .unwrap();
        for number in 0..MAX_SCOPED_MEMORY_FILES + 6 {
            std::fs::write(
                project
                    .dir()
                    .join("memory")
                    .join(format!("file-{number:03}.md")),
                format!("memory-{number}"),
            )
            .unwrap();
        }

        let scoped = scoped_context(&project, &target).unwrap();
        assert_eq!(scoped.memory_files.len(), MAX_SCOPED_MEMORY_FILES);
        assert_eq!(scoped.memory_files[0].0, "memory/file-000.md");
        assert_eq!(
            scoped.memory_files[MAX_SCOPED_MEMORY_FILES - 1].0,
            format!("memory/file-{:03}.md", MAX_SCOPED_MEMORY_FILES - 1)
        );
        assert!(
            scoped
                .memory_index
                .contains("later sorted files were not examined")
        );
        assert!(!scoped.memory_index.contains("SIBLING-MEMORY-SENTINEL"));
        assert!(
            scoped
                .memory_files
                .iter()
                .all(|(_, contents)| !contents.contains("SIBLING-MEMORY-SENTINEL"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn scoped_memory_never_follows_symbolic_link_files() {
        let world = World::new();
        let project = world.project("demo", "a.sock");
        let target = create_node(
            &project,
            &CreateNode {
                request: request(ROOT_ID, NodeRole::Worker),
                title: "Target".into(),
                kind: Kind::Tab,
                repo: String::new(),
                machine: String::new(),
                base: String::new(),
                task: "task".into(),
            },
        )
        .unwrap();
        let linked = project.dir().join("memory/linked.md");
        std::os::unix::fs::symlink("/etc/passwd", &linked).unwrap();

        let scoped = scoped_context(&project, &target).unwrap();
        assert!(scoped.memory_files.is_empty());
        assert!(
            scoped
                .memory_index
                .contains("memory/linked.md: refused non-regular file")
        );
        assert!(!scoped.memory_index.contains("root:x"));
    }

    fn write_scope_sentinel(project: &Project, id: &str, instruction: &str, memory: &str) {
        let scope = node_scope_dir(project, id);
        std::fs::write(scope.join("INSTRUCTIONS.md"), instruction).unwrap();
        std::fs::write(scope.join("MEMORY.md"), memory).unwrap();
        std::fs::write(scope.join("memory/scope.md"), format!("{memory}-FILE")).unwrap();
    }

    #[test]
    fn node_profiles_inherit_parent_values_and_allow_child_overrides() {
        let world = World::new();
        let project = world.project("demo", "a.sock");
        let coordinator = create_node(
            &project,
            &CreateNode {
                request: NodeRequest {
                    parent_id: ROOT_ID.into(),
                    role: NodeRole::Coordinator,
                    profile: ProfileOverrides {
                        harness: Some("codex".into()),
                        model: Some("gpt-5.6".into()),
                        reasoning_effort: Some("high".into()),
                        permission_profile: Some("workspace-write".into()),
                        raw_agent_args: vec!["--codex-arg".into()],
                    },
                    ..NodeRequest::default()
                },
                title: "Coordinator".into(),
                kind: Kind::Tab,
                repo: String::new(),
                machine: String::new(),
                base: String::new(),
                task: "coordinate".into(),
            },
        )
        .unwrap();
        let child = create_node(
            &project,
            &CreateNode {
                request: NodeRequest {
                    parent_id: coordinator.id.clone(),
                    role: NodeRole::Worker,
                    profile: ProfileOverrides {
                        model: Some("gpt-5.6-mini".into()),
                        raw_agent_args: vec!["$(touch remains-data)".into()],
                        ..ProfileOverrides::default()
                    },
                    ..NodeRequest::default()
                },
                title: "Child".into(),
                kind: Kind::Tab,
                repo: String::new(),
                machine: String::new(),
                base: String::new(),
                task: "work".into(),
            },
        )
        .unwrap();
        assert_eq!(child.agent, "codex");
        assert_eq!(child.model, "gpt-5.6-mini");
        assert_eq!(child.reasoning_effort, "high");
        assert_eq!(child.permission_profile, "workspace-write");
        assert_eq!(
            child.raw_agent_args,
            ["--codex-arg", "$(touch remains-data)"]
        );
        assert_eq!(
            AgentProfile {
                harness: child.agent.clone(),
                model: child.model.clone(),
                reasoning_effort: child.reasoning_effort.clone(),
                permission_profile: child.permission_profile.clone(),
                raw_agent_args: child.raw_agent_args.clone(),
            }
            .argv(&[])
            .unwrap()
            .last()
            .unwrap(),
            "$(touch remains-data)"
        );
        assert!(!child.can_spawn);
    }

    #[test]
    fn changing_harness_drops_raw_args_from_the_previous_cli() {
        let world = World::new();
        let project = world.project("demo", "a.sock");
        let coordinator = create_node(
            &project,
            &CreateNode {
                request: NodeRequest {
                    parent_id: ROOT_ID.into(),
                    role: NodeRole::Coordinator,
                    profile: ProfileOverrides {
                        harness: Some("codex".into()),
                        raw_agent_args: vec!["--codex-only".into()],
                        ..ProfileOverrides::default()
                    },
                    ..NodeRequest::default()
                },
                title: "Codex coordinator".into(),
                kind: Kind::Tab,
                repo: String::new(),
                machine: String::new(),
                base: String::new(),
                task: "coordinate".into(),
            },
        )
        .unwrap();
        let child = create_node(
            &project,
            &CreateNode {
                request: NodeRequest {
                    parent_id: coordinator.id,
                    role: NodeRole::Worker,
                    profile: ProfileOverrides {
                        harness: Some("claude".into()),
                        reasoning_effort: Some(String::new()),
                        permission_profile: Some("accept-edits".into()),
                        ..ProfileOverrides::default()
                    },
                    ..NodeRequest::default()
                },
                title: "Claude worker".into(),
                kind: Kind::Tab,
                repo: String::new(),
                machine: String::new(),
                base: String::new(),
                task: "work".into(),
            },
        )
        .unwrap();
        assert_eq!(child.agent, "claude");
        assert!(child.raw_agent_args.is_empty());
        assert!(
            AgentProfile {
                harness: child.agent,
                model: child.model,
                reasoning_effort: child.reasoning_effort,
                permission_profile: child.permission_profile,
                raw_agent_args: child.raw_agent_args,
            }
            .argv(&[])
            .unwrap()
            .iter()
            .all(|arg| arg != "--codex-only")
        );
    }

    #[test]
    fn legacy_node_records_are_loaded_without_rewriting() {
        let world = World::new();
        let project = world.project("demo", "a.sock");
        let path = thread::record_path(&project, "t-0001");
        std::fs::write(
            &path,
            "id = \"t-0001\"\ntitle = \"Legacy\"\nagent = \"claude\"\n",
        )
        .unwrap();
        let before = std::fs::read(&path).unwrap();
        let first = thread::load(&project, "t-0001").unwrap();
        let second = thread::load(&project, "t-0001").unwrap();
        assert_eq!(first, second);
        assert_eq!(first.parent_id, ROOT_ID);
        assert_eq!(first.role, NodeRole::Worker);
        assert!(!first.can_spawn);
        assert_eq!(std::fs::read(path).unwrap(), before);
        assert_eq!(tree(&project).unwrap()[0].thread.id, "t-0001");
    }
}

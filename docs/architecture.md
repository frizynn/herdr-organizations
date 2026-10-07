# Architecture and design decisions

## Scope

Herdr Organizations is a Rust 2024 modular monolith and a CLI-first plugin for Herdr. The CLI owns durable state transitions and delegates pane, worktree, agent and remote operations to the existing Herdr and Runner boundaries. There is no daemon, database, MCP server, web UI or harness-registered tool.

The project folder remains the unit of settings, lifecycle, locks, ticker, inbox, routines, reports and remote configuration. Its node hierarchy is stored with the existing thread records so placement and lifecycle state are not duplicated.

## Virtual root and node records

Every project has the virtual node `root`. It has no extra record. Its coordinator remains in the existing project coordinator state. Every child is a `Thread` record with `parent_id`, `role`, `can_spawn` and profile fields. The coordinator role uses the same record, placement, pane, ticker, report, worktree, inbox and remote behavior as a worker.

Serde defaults preserve old projects without a migration. A record without hierarchy fields loads as a worker directly below `root`, with no child-spawn permission. Reads do not rewrite the file. A tree load validates ids, duplicate records, missing parents, self-parenting, cycles, worker spawn permission and parent spawn permission before traversal or node creation. Traversal sorts numeric thread ids and emits deterministic preorder entries. Remote worker placement uses the existing SSH and remote Herdr flow. Creating or restarting a remote coordinator with `can_spawn=true` is refused before placement because its brief cannot safely invoke the local CLI; remote recursive coordinators need a future bridge.

Creation validates parent and profile before starting the ticker or contacting Herdr, then repeats validation while holding the per-project lock. When Herdr pane context identifies the root coordinator or a known node, the CLI also requires that coordinator to use itself as the requested parent and refuses workers. Calls from ordinary terminal sessions remain usable by the person operating the CLI. The record, node scope and task file are created together. Filesystem failure rolls back the new node state. A failed pane placement remains an individual failed node that can be retried without changing siblings.

## Scoped context

The project contributes `PROJECT.md`, `MEMORY.md` and sorted regular Markdown files in `memory/`. A node can add:

```text
nodes/<id>/INSTRUCTIONS.md
nodes/<id>/MEMORY.md
nodes/<id>/memory/*.md
```

A node brief composes project context, then each ancestor from root to parent, then the node's own files. Instructions and memory are deterministic. Siblings, descendants and other projects are excluded. The same composition is rebuilt on restart. Legacy nodes without a scope inherit project context only. Scoped memory reads allow at most 8 KiB per file, 32,000 bytes total and 64 files across the project and node ancestry. Omitted files are listed in the memory index. On supported Unix targets, each memory file is opened with `O_NOFOLLOW` on its final path component and checked through the opened file descriptor, closing the file-level metadata-check/open symlink race. Parent directories are checked separately and are not pinned against same-user replacement. Other platforms use a pre-open symlink check and are not listed as supported plugin targets.

Node scopes are treated as regular directories. Symbolic links for a node scope or its memory directory are refused, and symbolic-link memory files are skipped. This prevents a node brief from reading through a redirected scope.

`thread start --rules-file <file>` stores trimmed rules in `nodes/<id>/INSTRUCTIONS.md`, which the ancestor context includes for descendants. `thread rules <slug> <id>` reads those instructions; `--text-file <file>` replaces them for the next brief or restart. `node` is an alias of `thread`.

## Agent profiles

A thread launches from a named profile (harness, model, effort and extra arguments) that the user keeps in `config.toml`; agents only ever pass a name. A lead (`--role coordinator`) chooses from the project's coordinator profiles, a worker from its thread profiles, and the ticker checks the name against that allow-list again at every launch. Without `--profile`, the project's default for the role is used. Profiles are described in [operations](operations.md#agent-profiles).

Records from the first version of this fork also carried a model, an effort, a permission profile and raw arguments per node. Those fields are no longer read: such a node restarts on its harness's built-in profile unless `thread restart --profile <name>` picks another.

## Templates

Global templates are stored under `.templates/<name>/` and project templates under `<slug>/templates/<name>/`. `TEMPLATE.toml` stores the role, spawn permission and a profile name (older templates' harness and model fields are ignored), `RULES.md` stores node instructions without the generated heading, and `MEMORY.md` stores template-specific memory. When a project is selected, its template takes precedence over a global template with the same name. Without a project, resolution is global only. The commands are <code>template save &lt;name&gt;</code>, <code>template list</code>, <code>template show &lt;name&gt;</code>, <code>template memory &lt;name&gt;</code> and <code>template delete &lt;name&gt;</code>.

`thread start <slug> --template <name> --title ... --task-file ...` creates a node with the template's role, spawn permission, profile and rules. Explicit role, spawn and profile flags take precedence, and `--template` cannot be combined with `--rules-file`. The node record stores the template name so scoped context can include that template's current `MEMORY.md` for the node and its descendants, subject to the existing memory budget. Updating template memory changes future contexts for those nodes; siblings do not receive it. A missing template is ignored when building context. `RULES.md` is copied into the node's instructions at creation, so later changes to `RULES.md` do not change existing node instructions.

## Wake-ups

The ticker writes an inbox item for each event (a new report, a thread that needs someone, a pull request change, a due routine). Each item goes to the nearest open, local coordinator above the thread it is about, else to the project coordinator; routing reads records only, so `context` and the ticker agree. A lead that is away keeps its items until it is back, and its own state change goes to its parent.

An item whose class is listed in `quiet_events` in `PROJECT.md` (by default `idle`, `landing`, `resolved`, `pr-opened`, `pr-updated`) is written straight to `inbox/done/`: it stays as history and wakes nobody. The other items for a coordinator wait `wake_batch_secs` (90 by default) after the oldest of them, then for that coordinator to be idle for a minute with an input box that has looked empty for ten seconds. They then go out as one prompt that names subjects and fixed event phrases and quotes the first three lines of each new report, marked as data. Delivering the prompt archives the items, so the coordinator needs no follow-up command. The project coordinator's `context` lists only its own items and counts the rest per lead.

The thread brief asks every report to start with three lines, `PR:`, `Status:` and `Needs:`, and says not to prompt the coordinator directly. `PR: none` means no pull request.

## Heavy commands

`gate run [--name heavy] [--slots 1] -- <command>` queues a command per machine. A slot is an exclusive lock on `<root>/.gates/<name>-<n>.lock`, released by the operating system when the holder exits, so a killed thread never blocks the queue. Before running, the command also waits while the kernel reports memory pressure (macOS `kern.memorystatus_vm_pressure_level` at warn or critical, Linux PSI `some avg10` above 10%). The holder writes its pid, start time, folder and command into its lock file, which a waiting command prints once. The gated command inherits the terminal and its exit code is passed through. Thread briefs ask for it around full test suites, builds and browser runs.

## Project tree

The `organization-sidebar` action resolves its project from the current Herdr workspace and opens the project's tree in a split pane. Upstream's projects popup (`prefix+a`) stays the place for tasks, inbox, routines, settings and memory; the tree is for watching and moving between agents.

The first row under the title summarises the project (`16 nodes · 2 need you · 5 working · 3 PRs open`). Each row shows a node's title, the ticker's state line (`working · ~40%`, `review · report`), its pull request (`PR #12 open`) and, for leads, their role. The three rows under the tree show the selected node's report header, or for the root the ids that need the user. Enter focuses the node's agent, reopening an open node whose pane is gone; Space folds a lead; `n` jumps to the next node that needs the user; `x` twice closes the panes of a node and everything under it; `s` opens settings. Resolved leaves are hidden unless the settings show them; a resolved lead stays while it has open work under it.

Everything a row shows comes from records and report files the ticker keeps, so the first frame needs no live Herdr call. A plugin-owned metadata token plus project and workspace tokens identify each split. The sticky open state maps tabs to sidebar panes per workspace and session socket, so returning to a visited tab does no layout work. Closing from the shortcut, `q` or Esc clears that set. Frames are diffed per row and written in synchronized updates.

## Compatibility and intentional limits

- `~/.herdr-projects/`, `~/.config/herdr-projects/` and `HERDR_PROJECTS_ROOT` remain unchanged, and so do the plugin id and the `herdr-projects` binary, so hooks and `AGENTS.md` files that call it keep working.
- `thread` is the one command group for workers and leads; `node` is an alias.
- Existing ticker cadence, inbox format, reports, routines, worktree behavior and SSH transport remain in place.
- Remote workers remain supported. Remote coordinators with child-spawn permission are refused until a remote CLI bridge exists.
- `can_spawn` is deterministic CLI validation, not an ACL. Agents still have the shell and permissions of their harness.
- The plugin does not register tools with Codex, Claude or another harness. A coordinator invokes the CLI through its normal shell permission system.

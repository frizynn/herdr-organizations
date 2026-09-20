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

## Agent profiles

Each node stores the effective harness, model, reasoning effort, permission profile and raw argv components inherited from its parent. Raw argv components are inherited only while the harness stays the same. Creation can override any supported value. Repeatable `--raw-agent-arg` values remain separate argv entries. The implementation never turns profile fields into a shell command string.

Codex adapter:

- `--model <value>` maps to `--model <value>`.
- Reasoning effort accepts `minimal`, `low`, `medium`, `high`, `xhigh` and `max`, mapped to `--config model_reasoning_effort="<value>"`.
- `read-only` maps to `--sandbox read-only --ask-for-approval on-request`.
- `workspace-write` maps to `--sandbox workspace-write --ask-for-approval on-request`.
- `full-access` maps to `--sandbox danger-full-access --ask-for-approval never`.

Claude Code adapter:

- `--model <value>` maps to `--model <value>`.
- Permission profiles `default`, `plan`, `accept-edits` and `bypass-permissions` map to Claude Code's `--permission-mode` values.
- The adapter does not map reasoning effort. Supply a harness-specific option as a raw argv component when needed.

Other harness kinds receive raw argv only. Built-in profile fields are rejected for harnesses without an adapter. The `herdr agent start` boundary receives the harness kind and exact argv vector. Project-wide `thread_agent_args` remain appended after profile arguments for compatibility, but conflicting permission, sandbox and approval flags are rejected in raw and project safety args when a permission profile is selected. This checks argv values only. It does not provide OS isolation or restrict an agent's shell access.

These flags are adapters for current CLI interfaces, not a promise that all agent CLIs accept the same options. See the [Codex sandbox documentation](https://developers.openai.com/es-419/docs/sandboxing), [Codex configuration reference](https://developers.openai.com/es-419/docs/config-file/config-advanced), and [Claude Code CLI reference](https://docs.anthropic.com/en/docs/claude-code/cli-usage).

## Organization picker

The `organizations` action remains the global project picker popup. The distinct `organization-sidebar` action resolves its project from the current Herdr workspace and opens a recursive tree in a split pane. Its compact terminal UI supports keyboard navigation, coordinator disclosure, status colors and an in-pane settings screen. Selecting a node focuses its live agent, focuses its existing tab when no agent is attached, or runs the existing restart mechanic when its active tab is gone.

The popup starts with a project picker, then shows the virtual root and descendants in tree order with title, state, role and id. The split sidebar stays within one project, leads rows with titles and can hide role/status or include resolved nodes. Resolved leaves stay in durable history but are hidden by default. Its settings are stored under the Herdr-provided plugin config directory. A plugin-owned metadata token plus project and workspace tokens identify the split; the toggle only focuses or closes panes matching all three values in that workspace. The sidebar heartbeats the tokens while open. Both terminal views use a stable leading selection marker, restrained status color and synchronized updates instead of reverse-video rows or unsynchronized full-screen redraws.

The pure tree layout, collapse, settings and selection helpers are unit-testable without a terminal. Actual split geometry, terminal colors and key forwarding remain client-side checks.

## Compatibility and intentional limits

- `~/.herdr-projects/`, `~/.config/herdr-projects/` and `HERDR_PROJECTS_ROOT` remain unchanged.
- `herdr-organizations` is the primary binary. Cargo also builds `herdr-projects` as a compatibility name.
- `thread start` remains a direct worker-under-root alias. Node lifecycle commands provide the hierarchy-aware names.
- Existing ticker cadence, inbox format, reports, routines, worktree behavior and SSH transport remain in place.
- Remote workers remain supported. Remote coordinators with child-spawn permission are refused until a remote CLI bridge exists.
- `can_spawn` is deterministic CLI validation, not an ACL. Agents still have the shell and permissions of their harness.
- The plugin does not register tools with Codex, Claude or another harness. A coordinator invokes the CLI through its normal shell permission system.

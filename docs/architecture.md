# Architecture and design decisions

## Scope

Herdr Organizations is a Rust 2024 modular monolith and a CLI-first plugin for Herdr. The CLI owns durable state transitions and delegates pane, worktree, agent and remote operations to the existing Herdr and Runner boundaries. The only resident process is the ticker. There is no database, MCP server, web UI or harness-registered tool.

The project folder remains the unit of settings, lifecycle, locks, ticker, inbox, routines, reports and remote configuration. Its records (reports, task files, Next lines, libraries and status files) may live in another folder that `records` in `PROJECT.md` names; `Project::records_dir` resolves it and the path helpers in `records.rs` are the only place that builds those paths. Its node hierarchy is stored with the existing thread records so placement and lifecycle state are not duplicated.

## Virtual root and node records

Every project has the virtual node `root`. It has no extra record. Its coordinator remains in the existing project coordinator state. Every child is a `Thread` record with `parent_id`, `role`, `can_spawn` and profile fields. The coordinator role uses the same record, placement, pane, ticker, report, worktree, inbox and remote behavior as a worker.

Serde defaults preserve old projects without a migration. A record without hierarchy fields loads as a worker directly below `root`, with no child-spawn permission. Reads do not rewrite the file. A tree load validates ids, duplicate records, missing parents, self-parenting, cycles, worker spawn permission and parent spawn permission before traversal or node creation. Traversal sorts numeric thread ids and emits deterministic preorder entries. Remote worker placement uses the existing SSH and remote Herdr flow. Creating or restarting a remote coordinator with `can_spawn=true` is refused before placement because its brief cannot safely invoke the local CLI; remote recursive coordinators need a future bridge.

A thread's routine events (a new report, idle, landing, resolved) write no project inbox item when its direct parent is an open, local coordinator node that may spawn and the project's `nudge` is on. The ticker queues what that node acts on in `.state/ticker.json` (a new report, landing, and a child that stopped working without a new report) and wakes it with one code-generated line (a child is named by its report's verdict when it has one, the only report value that reaches a prompt). Idle after an acknowledged report and resolved wake nobody. Waiting on you goes to both. Under `root`, with a parent that is resolved, failed, remote or missing, or with `nudge` off (nothing would wake the node), the item goes to the project inbox as before, so nothing is lost. The root coordinator learns that a child coordinator finished from that coordinator's own report. Pull request, task and outage items are not routed this way.

Creation validates parent and profile before starting the ticker or contacting Herdr, then repeats validation while holding the per-project lock. When Herdr pane context identifies the root coordinator or a known node, the CLI also requires that coordinator to use itself as the requested parent and refuses workers. Calls from ordinary terminal sessions remain usable by the person operating the CLI. The record, node scope and task file are created together. Filesystem failure rolls back the new node state. A failed pane placement remains an individual failed node that can be retried without changing siblings.

## Scoped context

The project contributes `PROJECT.md`, `MEMORY.md` and sorted regular Markdown files in `memory/`. A node can add:

```text
nodes/<id>/INSTRUCTIONS.md
nodes/<id>/MEMORY.md
nodes/<id>/memory/*.md
```

A node brief composes project context, then each ancestor from root to parent, then the node's own files. Instructions and memory are deterministic. Siblings, descendants and other projects are excluded. The same composition is rebuilt on restart. Legacy nodes without a scope inherit project context only. Scoped memory reads allow at most 8 KiB per file, 32,000 bytes total and 64 files across the project and node ancestry. Omitted files are listed in the memory index. On supported Unix targets, each memory file is opened with `O_NOFOLLOW` on its final path component and checked through the opened file descriptor, closing the file-level metadata-check/open symlink race. Parent directories are checked separately and are not pinned against same-user replacement. Other platforms use a pre-open symlink check and are not listed as supported plugin targets.

A local coordinator node that may spawn gets `skill/COORDINATOR_NODE.md` in place of the thread brief: it coordinates its area and hands all hands-on work to children. Its own status file, `nodes/<id>/STATUS.md` in the records folder, which it keeps up to date, is inlined after its memory under `# Your status`, on start and on restart. It is read with the memory file rules (regular file, `O_NOFOLLOW`, at most 8 KiB) but outside the memory budget, and left out above 4,000 characters, the root's limit too, with the reason in its place. Only that node's brief reads it: descendants never inherit it, and a template saved from the node does not copy it. Leaf coordinators and workers keep the thread brief.

The root coordinator's status file is the project's `STATUS.md`, in the records folder. `--json` and the popup read a status file as a regular file opened with `O_NOFOLLOW` and cut to 4,000 characters. On a prompt the user typed in the root's pane or a spawning coordinator node's pane, the progress hook stores that time, never the text, in `.state/asked.json` under the project lock, keeping the last two per coordinator (`root` or the node id). A status file older than the previous one is stale: `context` lists `status-stale` for the root, and the node's hook adds one line to the prompt.

A node template (`templates::Template`) stores a role, spawn permission, profile name, `RULES.md` and the same memory layout as a node scope, under `<config>/templates/<name>/` or `<project>/templates/<name>/`. `node start --template` fills the unset parts of the `NodeRequest`, and `create_node` writes the template's memory into the new scope instead of an empty `MEMORY.md`. Nothing links the node to the template afterwards, so scoped context is unchanged.

`node start --rules-file <file>` writes the trimmed rules (at most 8 KiB) into the new node's `INSTRUCTIONS.md` under the same lock as the record, so the first brief already has them. `node rules <slug> <id>` prints them and `--text-file <file>` replaces them; a running agent sees the change on its next brief or restart, as its descendants do.

A thread's report also carries what a reader needs without opening it (`report.rs`): `--json`, `node show` and the popup read the verdict of its `Status:` line, its `## Summary`, the images listed under `## Captures` and the documents at the top of `library/<id>/`. A capture path must name a regular image file inside the library through real folders; `..`, absolute paths and symbolic links anywhere on the way are dropped. These checks use `lstat` and are not pinned, so a same-user swap between the check and a client's read is not prevented. Only the verdict, a fixed value, ever reaches a prompt.

Node scopes are treated as regular directories. Symbolic links for a node scope or its memory directory are refused, and symbolic-link memory files are skipped. This prevents a node brief from reading through a redirected scope.

## Records and switches

`records` moves only by `set`: it copies every record under the project lock, keeping modification times, writes the setting, and removes the originals once both succeeded, so a failure leaves the records where they were. Reading never migrates anything. Thread records, node scopes, the inbox and `.state/` stay in the project folder because the ticker and the CLI take the project lock over them; a synced folder could rewrite them behind that lock.

`progress` and `turn_reminder` are read by the hook on each event and by brief composition, so a change reaches the next event or brief without a restart. With `progress` off the hook answers no progress event. With `turn_reminder` off the hook still records the prompt's time in `.state/asked.json`. `nudge` is read by the ticker on each pass.

## Agent profiles

A node launches with a named profile from Herdr Projects (`[profiles.<name>]` in `config.toml`, or a Herdr agent kind as a built-in profile): `--profile`, else its parent node's, else the project default for its role, checked against that role's allow-list at creation and at every launch. The profile decides the harness. On top of it each node stores model, reasoning effort, permission profile and raw argv components inherited from its parent; the node's model and effort replace the profile's, and a node with its own permission profile gets no yolo flags. Raw argv components are inherited only while the harness stays the same. Creation can override any supported value. Repeatable `--raw-agent-arg` values remain separate argv entries. The implementation never turns profile fields into a shell command string.

Model and effort use the profile flags' own adapter (`--model`; Codex `-c model_reasoning_effort="<value>"`, Claude Code and Copilot `--effort`, pi and oh-my-pi `--thinking`). Permission adapters:

Codex:
- `read-only` maps to `--sandbox read-only --ask-for-approval on-request`.
- `workspace-write` maps to `--sandbox workspace-write --ask-for-approval on-request`.
- `full-access` maps to `--sandbox danger-full-access --ask-for-approval never`.

Claude Code:

- Permission profiles `default`, `plan`, `accept-edits` and `bypass-permissions` map to Claude Code's `--permission-mode` values.

Other harness kinds receive raw argv only. Built-in profile fields are rejected for harnesses without an adapter. The `herdr agent start` boundary receives the harness kind and exact argv vector. Project-wide `thread_agent_args` remain appended after profile arguments for compatibility, but conflicting permission, sandbox and approval flags are rejected in raw and project safety args when a permission profile is selected. This checks argv values only. It does not provide OS isolation or restrict an agent's shell access.

These flags are adapters for current CLI interfaces, not a promise that all agent CLIs accept the same options. See the [Codex sandbox documentation](https://developers.openai.com/es-419/docs/sandboxing), [Codex configuration reference](https://developers.openai.com/es-419/docs/config-file/config-advanced), and [Claude Code CLI reference](https://docs.anthropic.com/en/docs/claude-code/cli-usage).

## Merge policy

`steps::merge_pull_request` is the one merge path for `thread merge`, the popup and auto-merge. It reads the pull request again, applies the invariants that hold under every policy (`pr::merge_blocker`: open, not a draft, no conflict, no reported check failed or unfinished, a known head commit; then the thread's own branch, repository and base), and then the policy. Under GitHub's policy, the default, the review must approve and at least one check must be reported. A project with `merge_gate` in its safety table replaces those two with its own command (`merge_gate.rs`), run as argv through the Runner just before `gh pr merge`, on the head commit the merge is pinned to with `--match-head-commit`. Its verdict per thread and head commit lives in `.state/merge-gate.json`, written under the project lock because the CLI, the popup and the ticker all write it, so `--json`, the popup and the Landing group show it without running the command. Its output is user text: one cleaned line reaches the inbox, `--json` and the popup, never a prompt. The merge settings are read from `config.toml` only and `safety set` refuses them, `default` included, so an agent cannot change them through the CLI. The gate's program must be absolute or a bare name on `PATH`, since a relative one would resolve in a folder agents write, and a `config.toml` that cannot be read fails the merge instead of reading as empty.

## Terminal UI and refresh

One process stays resident: the ticker. It holds one `events.subscribe` connection per project socket (workspace, tab and pane lifecycle plus `pane.agent_status_changed` for each agent pane) and runs a pass when an event arrives, after a 250 ms settle. Without events it reconciles every 60 s, every 15 s while a launch or brief is pending, and exactly when a blocked agent's 30 s debounce ends. Events are wake-up signals only; every pass re-reads state, so a missed event costs one reconcile. A subscription that fails (a pane closed between the list and the subscribe, `events_lost`) is rebuilt with backoff, and Settings shows it as down.

After each pass the ticker builds one view model (`state.rs`) and writes `<root>/.organizations-state.json` only when its content changed. From the same model it reports the organization tokens (`tokens.rs`) over the socket (`pane.report_metadata`, `workspace.report_metadata`) with a 300 s TTL, sending a value only when it changed or half its TTL passed. Agent and pane lists for the model use socket requests too. The upstream row report (display name, the "needs you" label and the `hp_*` grouping tokens, `sidebar.rs` and `grouping.rs`) still goes through Herdr's CLI, once per pane per pass. Metadata events are not subscribed, so the ticker's own reports never wake it.

Every Organizations screen is one TUI (`ui.rs`) in the `ui` popup; upstream's Projects popup (`popup.rs`, pane `projects`) stays a separate popup for tasks, profiles and safety. Herdr shows one popup at a time, so the merge confirmation is a dialog inside the same process. The TUI blocks on terminal input or a kqueue/inotify wake on the projects root (`watch.rs`); it has no timer. It calls Herdr only for an action the user asked for (`ui_ops.rs`), reusing the CLI mechanic of the same name: go to pane is `focus`/`restart`, merge is `thread merge` plus `node resolve`, New is `new`, `node start` or `workspace create`. Herdr keeps a popup modal until its process exits, so a step that can move focus and wait for an agent to start (go to pane, the `open` picker, a new project's coordinator) is a `ui_ops::Jump`: the popup closes and a detached `jump` process makes it once the popup's process has exited. Without a running ticker the popup builds the model once from the records and four socket reads.

The optional dock runs the same TUI in a split. The toggle finds it by a stored pane id plus identity tokens the dock reports once without a TTL, so a pane id reused after a server restart is never closed and no heartbeat is needed.

`sidebar install` edits Herdr's config with `toml_edit`: it appends rows that name the tokens and never edits or removes existing rows. Panel headers are drawn by Herdr outside `rows`, so header and row clicks keep working; this was checked in a disposable Herdr (docs/herdr-notes.md, 2026-10-10).

Keybindings and view settings live in `~/.config/herdr-projects/tui.toml`. Commands are scoped to screens, so a key may be shared only by commands that never appear on the same screen; conflicts are refused in Settings and reported for hand edits.

## Compatibility and intentional limits

- `~/.herdr-projects/`, `~/.config/herdr-projects/` and `HERDR_PROJECTS_ROOT` remain unchanged.
- `herdr-organizations` is the primary binary. Cargo also builds `herdr-projects` as a compatibility name; it is the same program. Paths written into hooks, the tab bar, `AGENTS.md` and the `~/.local/bin` link name `herdr-projects`, so entries written by Herdr Projects keep working and are recognized as this plugin's.
- Herdr Projects 0.2.34 features are kept as they are: the sidebar grouping (`hp_*` tokens, written by `configure`), the Projects popup (`projects` pane), progress hooks, profiles, tasks, cleanup on resolve, rename, update. Where both changed the same thing the upstream mechanism wins and the organization feature sits on top: brief delivery is upstream's `brief::deliver`, the nudge is upstream's `[hp inbox]` line, notifications are upstream's `Notifier` filtered by the popup's notify setting, and organization tokens no longer use the `project`/`thread`/`review`/`rank` names that upstream clears.
- `thread start` remains a direct worker-under-root alias. Node lifecycle commands provide the hierarchy-aware names.
- Existing ticker cadence, inbox format, reports, routines, worktree behavior and SSH transport remain in place.
- Remote workers remain supported. Remote coordinators with child-spawn permission are refused until a remote CLI bridge exists.
- `can_spawn` is deterministic CLI validation, not an ACL. Agents still have the shell and permissions of their harness.
- The plugin does not register tools with Codex, Claude or another harness. A coordinator invokes the CLI through its normal shell permission system.

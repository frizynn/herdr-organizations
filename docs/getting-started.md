# Getting started

Build or link the plugin, create a project and open its root coordinator. Local coordinators can create nested coordinator or worker nodes. Remote workers remain supported; remote recursive coordinators with child-spawn permission are refused until a remote CLI bridge is available.

## Prerequisites

- macOS or Linux
- Herdr 0.9.1 or newer on both the client and server
- Rust and Cargo 1.89 or newer
- Git and an agent CLI kind supported by `herdr agent`
- Optional: `gh` for pull request follow-up, plus `ssh` and `rsync` for remote machines

## Link and build the plugin

From the repository root, replace an existing upstream registration first, then build and link this checkout:

```sh
herdr plugin uninstall herdr-projects
cargo build --release --locked
herdr plugin link .
```

The plugin id remains `herdr-projects` to preserve its config and project store. Its visible name is **Herdr Organizations**. Herdr builds the plugin from its manifest when it links it. `herdr plugin list` confirms the `herdr-projects` registration. To run the CLI directly after the build, use `target/release/herdr-organizations`. A `herdr-projects` binary is also built for scripts that still use the old name.

## Create and open a project

Press `n` in the Organizations popup, or use the CLI:

```sh
target/release/herdr-organizations new "Billing" --goal "Ship the new billing page" --repo ~/dev/billing
target/release/herdr-organizations open billing
```

The project is created at `~/.herdr-projects/billing/` by default. `open` creates its Herdr workspace and root coordinator pane. The coordinator prints the CLI prefix and loads the coordinator skill. If your agent asks whether it can trust the project folder, answer in that pane. The ticker sends the initial prompt when the agent is ready.

Project settings and root instructions live in `PROJECT.md`. `thread_agent` selects the default worker harness, `coordinator_agent` selects the root coordinator harness, and `max_parallel_threads` limits active project nodes. Existing settings in `~/.config/herdr-projects/config.toml` continue to work.

The root coordinator maintains `HANDOFF.md` as a compact cross-harness handoff. A fresh Codex or Claude coordinator receives project instructions, that handoff, the memory index, tasks and the open organization in stable tree order through `context`. Automated ticker turns use `inbox consume` instead, so a state notification does not reload the whole project or require a separate acknowledgement command.

## Create nodes

The root coordinator is the virtual parent `root`. A worker is a leaf. A local coordinator can create workers or more coordinators beneath itself. A local child coordinator brief gives it the project CLI prefix and tells it to use its own node id as parent. A remote coordinator cannot receive child-spawn permission because its brief cannot safely launch the local CLI from the remote machine.

Create a worker under the root:

```sh
target/release/herdr-organizations node start billing \
  --parent root \
  --role worker \
  --title "Add billing validation" \
  --repo ~/dev/billing \
  --task-file - <<'TASK'
Add server-side validation for billing addresses. Run the relevant tests and report the changes.
TASK
```

Create a coordinator to plan a large subproject:

```sh
target/release/herdr-organizations node start billing \
  --parent root \
  --role coordinator \
  --title "Billing API" \
  --repo ~/dev/billing \
  --task-file - <<'TASK'
Plan the billing API. Delegate implementation tasks to children beneath your own node id.
TASK
```

`--repo` creates a worktree. Without it, the node runs in a tab in the project workspace. Add `--machine <label>` for a repository on a saved SSH machine and `--base <ref>` for a specific base branch. Use `--no-spawn` when creating a coordinator that should not create children.

Profile fields inherit from the parent unless they are specified on node creation. Choose `--harness`, `--model`, `--reasoning-effort` and `--permission-profile`. Codex and Claude have separate CLI adapters. Use repeatable `--raw-agent-arg` options for harness-specific argv components that do not have a built-in adapter. Project-wide `thread_agent_args` remain separate argv values; permission and sandbox override flags are rejected when they conflict with a selected profile. This is argv validation, not OS isolation.

The existing `thread start` command remains a worker-under-root alias. Use `node restart`, `node prompt`, `node list`, `node show`, `node ack` and `node resolve` for hierarchy-aware names. `node create` is an alias for `node start`.

Resolving a node and closing its terminal surface are intentionally distinct. Use `node resolve <project> <id> --close-view` when finished work should disappear from both the organization tree and Herdr's tabs or workspaces. The branch, worktree and copied report remain available. Removing a worktree still requires the separate `--remove-worktree` option.

## The Organizations popup

Run **Herdr Organizations: launcher** from Herdr's action menu, or bind it to a key (below). Every interactive screen is one small TUI inside a Herdr popup. It exists only while the popup is open; Esc goes back, then closes it.

- **Launcher**: "needs you" across every coordinator (oldest first), then each project with its coordinators nested and their counts, then workspaces outside a project. `1`-`9` jump to a coordinator, `n` new project, `c` new coordinator, `a` turns the current workspace into a project, `/` filters, `b` opens the board, `s` settings.
- **New** (`n`): one form for a project, coordinator, thread or workspace. Up/Down switch what to create, Tab moves between fields, Left/Right choose, Enter creates. The last line shows the exact CLI call the form makes.
- **Project tree** (Enter on a project or coordinator): project › coordinators › threads. Enter goes to the selected pane, Space folds a coordinator, `t` starts a thread under it, `m` merges a reviewed pull request, `1`-`9` press that option in the pane of an agent that is waiting on you.
- **Board** (`b`): every thread in four columns (needs you, working, review, resolved), idle threads on one line and the running coordinators below. `f` cycles the coordinator filter.
- **Thread detail** (Enter on a card or a needs-you row): the question or the agent's current line, the PR and its checks, the thread's inbox items (`d` marks one done) and the last lines of its pane, read once.
- **Merge confirm** (`m`): drawn by the same process, because Herdr shows one popup at a time. It merges only when the pull request passes the same guard as `thread merge`, then resolves the thread and tells its coordinator. `k` keeps the worktree.
- **Settings** (`s`): dock side and width, resolved threads as a count or a list, which transitions show a Herdr notification, every keybinding, and the ticker's health.

The popup reads `~/.herdr-projects/.organizations-state.json`, which the ticker rewrites when something changes, and redraws on a key or a change of that file. It never polls Herdr. Without a running ticker it builds the same view from the records once when it opens.

### Keybindings

Defaults follow the design: `↵` open, `n` new project, `c` new coordinator, `t` new thread, `b` board, `m` merge, `1`-`9` jump or answer, `s` settings, `/` search, `esc` back. Rebind any command in Settings (select it, press Enter, press the new key; Backspace restores the default) or edit `~/.config/herdr-projects/tui.toml`:

```toml
[view]
dock = "right"        # off, right or left
dock_width = 30       # 15-50
resolved = "count"    # count or list
notify = "needs-you-and-review"  # or needs-you, off

[keys]
board = "w"
up = ["up", "k"]
```

Only changed keys are written. A key that two commands on the same screen would share is refused, and a bad entry in the file keeps its default and is listed under KEYS in Settings.

### Launcher key

Herdr keybindings stay in Herdr's own config. Add this to `~/.config/herdr/config.toml`, then reload Herdr's config:

```toml
[[keys.command]]
key = "prefix+a"
type = "plugin_action"
command = "herdr-projects.organizations"
description = "Organizations"

[[keys.command]]
key = "prefix+shift+y"
type = "plugin_action"
command = "herdr-projects.organization-sidebar"
description = "Organizations dock"
```

## What stays on screen without a plugin process

The ticker reports display tokens and Herdr draws them in its own sidebar:

- each coordinator's workspace gets `$org_need`, `$org_work` and `$org_review` (for example `●2`), hidden when zero;
- each agent pane gets `$org_task` (its coordinator, `review #1342`, `merged #1338`) and the state label "needs you" instead of "blocked";
- **Herdr Organizations: focus sidebar on this project** filters the agents list to one project; the short label sits where the sort word was, so the `agents` header stays.

Herdr only renders tokens that its sidebar layout names. Add the rows once:

```sh
target/release/herdr-organizations sidebar install --dry-run   # print the edited config
target/release/herdr-organizations sidebar install             # write it, keeping a dated backup
```

It appends rows to `[ui.sidebar.spaces]` and `[ui.sidebar.agents]` (and `state_text` when no row shows the state). It never edits or removes your rows and never touches the panel headers (`spaces`, `new`, `menu`, `agents`, the sort word), which Herdr draws outside `rows`. Reload Herdr's config afterwards.

## The optional dock

**Herdr Organizations: toggle dock** opens a split next to the current workspace's coordinator: needs you, review, working, idle, resolved count, inbox and a new-thread row. It is off by default; turn it on in Settings first. It reads the same state file and never polls Herdr. Enter goes to a pane and keeps the dock, `m` merges, `t` opens the New form in the popup, `?` lists the keys. The same action closes it.

## Preserve existing projects

The binary continues to read projects from `~/.herdr-projects/`, settings from `~/.config/herdr-projects/`, and `HERDR_PROJECTS_ROOT`. Legacy thread records are loaded as workers below `root` without rewriting them. Existing reports, worktrees, routines, ticker state, inbox items and remote settings remain in their current paths.

For operational details and agent permission guidance, see [Operations](operations.md). Use the [manual test guide](manual-test.md) for the keyboard, mouse and live-pane checks that require a Herdr client.

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

Run **Herdr Organizations: new project** from the Herdr action menu, or use the CLI:

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

## Browse and focus the tree

Run **Herdr Organizations: organization tree** from Herdr's action menu to browse all projects. The popup first lists projects, then renders the selected project root and all descendants with role and state.

- Up and Down or `k` and `j` move the selection.
- Enter opens a project's tree. Inside the tree it focuses a live agent, opens an existing tab, or recreates an active node whose tab was closed. The popup closes after a successful open.
- Esc or `q` goes back or closes the popup.
- `r` refreshes the current tree.
- A mouse click selects a row. A double-click opens or focuses it when the Herdr client forwards terminal mouse events.

Inside a project workspace, run **Herdr Organizations: toggle project hierarchy sidebar** to show a right split scoped to that project. It defaults to 30% width and does not steal focus on open.

The open state is sticky across tabs in the same workspace. The first visit to a tab creates its local split in the background; later switches reuse that already-rendered split and select the coordinator or worker for the active tab. Closing with the shortcut, `q`, or Esc closes the complete sticky set.

- Up and Down or `j` and `k` move through visible rows; Enter focuses or reopens a node while leaving the tree available.
- Space folds or expands a coordinator. `s` opens the visible gear/settings surface.
- `q` or Esc closes the split. Settings can move the dock, change width, control auto-open and focus behavior, include resolved nodes, hide status or role, and change strict toggle behavior.
- Herdr keybindings remain user-managed. The settings screen only controls the contextual sidebar and keeps command ids out of the normal navigation flow.

To bind the global picker and project hierarchy without starting a shell subprocess, add this to `~/.config/herdr/config.toml`, then run `herdr server reload-config`:

```toml
[[keys.command]]
key = "prefix+shift+o"
type = "plugin_action"
command = "herdr-projects.organizations"
description = "Open Herdr Organizations project picker"

[[keys.command]]
key = "prefix+shift+y"
type = "plugin_action"
command = "herdr-projects.organization-sidebar"
description = "Toggle Herdr Organizations hierarchy sidebar"
```

With Herdr's default prefix, press `Ctrl+B`, release it, then press `Shift+O` or `Shift+Y`. `Shift+H` is intentionally avoided because Herdr already uses `prefix+shift+h` to swap the active pane left.

Sidebar settings and the sticky per-tab pane map are saved to `organization-sidebar.json` in Herdr's `HERDR_PLUGIN_CONFIG_DIR`. Auto-open is off by default; an explicitly open sticky sidebar still follows newly visited tabs after that setting is disabled. Project, workspace and Herdr session metadata are checked before any cached pane is reused.

## Preserve existing projects

The binary continues to read projects from `~/.herdr-projects/`, settings from `~/.config/herdr-projects/`, and `HERDR_PROJECTS_ROOT`. Legacy thread records are loaded as workers below `root` without rewriting them. Existing reports, worktrees, routines, ticker state, inbox items and remote settings remain in their current paths.

For operational details and agent permission guidance, see [Operations](operations.md). Use the [manual test guide](manual-test.md) for the keyboard, mouse and live-pane checks that require a Herdr client.

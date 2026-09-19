# Herdr Organizations

Herdr Organizations adds recursive coordinator and worker hierarchies to [Herdr](https://herdr.dev). A project is the virtual root. Local coordinators can create coordinator or worker children at any depth, while workers are leaves. Remote workers remain supported. A remote coordinator with child-spawn permission is refused until a remote CLI bridge is available.

Project instructions and memory flow from the root through a node's ancestors and into that node. Siblings and descendants are excluded. Node creation stores its parent, role, spawn permission, harness profile and task alongside the existing thread lifecycle record.

## Start here

- [Getting started](docs/getting-started.md)
- [Operations and hierarchy reference](docs/operations.md)
- [Architecture and design decisions](docs/architecture.md)
- [Manual validation](docs/manual-test.md)
- [Contributing](CONTRIBUTING.md)
- [Security policy](SECURITY.md)

## Quick example

```sh
cargo build --release --locked
target/release/herdr-organizations new "Billing" --repo ~/dev/billing
target/release/herdr-organizations open billing
```

In the root coordinator pane, describe the outcome. After planning, it can create a coordinator for a large subproject or a worker for a bounded task. The CLI shape is:

```sh
target/release/herdr-organizations node start billing \
  --parent root \
  --role coordinator \
  --title "Billing API" \
  --repo ~/dev/billing \
  --harness codex \
  --model gpt-5.6 \
  --reasoning-effort high \
  --permission-profile workspace-write \
  --task-file - <<'TASK'
Plan and implement the billing API. Delegate bounded implementation tasks to children beneath your own node id.
TASK
```

New descendants inherit omitted profile fields from their parent. Use `--no-spawn` for a coordinator that must remain a leaf. Permission profiles validate the harness argv; they are not OS isolation. `thread start` remains a backward-compatible way to start a worker directly under the project root.

## Organization tree

Herdr's **Herdr Organizations: organization tree** action opens a popup that lists projects and then shows the selected project's root and descendants. Use Up/Down or `j`/`k` to move, Enter to open or focus, Esc or `q` to go back or close, and `r` to refresh. Mouse selection and double-click focus work when the Herdr client forwards terminal mouse events. The popup explicitly focuses the selected live pane. The tree runs in the popup process because Herdr's native sidebar exposes a flat agent list.

Pane metadata includes project, node id, role, parent, depth, tree order and current review group. Existing overview, focus, open, inbox, routines, remote machines and reports continue to use the project root and thread records.

## Compatibility

The primary package, repository and binary are `herdr-organizations`. Cargo also builds the `herdr-projects` compatibility binary. The plugin id remains `herdr-projects`, while its visible name is **Herdr Organizations**. When replacing upstream, uninstall its registration first with `herdr plugin uninstall herdr-projects`, then build and link this checkout with `cargo build --release --locked` and `herdr plugin link .`. Existing projects remain under `~/.herdr-projects/`, and settings remain under `~/.config/herdr-projects/`. Legacy thread records load as worker nodes below `root`. Loading does not rewrite them. The CLI accepts the existing `HERDR_PROJECTS_ROOT` environment variable and configuration format.

This fork retains the upstream MIT license and attribution. See [NOTICE](NOTICE).

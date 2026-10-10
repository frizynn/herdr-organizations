# Herdr Organizations

Herdr Organizations adds recursive coordinator and worker hierarchies to [Herdr](https://herdr.dev). It is a superset of [Herdr Projects](https://github.com/eliasstravik/herdr-projects) 0.2.34: every command, setting and project file of that release works the same, and the organization features sit on top. A project is the virtual root. Local coordinators can create coordinator or worker children at any depth, while workers are leaves. Remote workers remain supported. A remote coordinator with child-spawn permission is refused until a remote CLI bridge is available.

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
target/release/herdr-organizations configure   # sidebar rows, popup key, progress hooks, /autoproject skill
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
  --profile codex \
  --model gpt-5.6 \
  --reasoning-effort high \
  --permission-profile workspace-write \
  --task-file - <<'TASK'
Plan and implement the billing API. Delegate bounded implementation tasks to children beneath your own node id.
TASK
```

A node's profile is `--profile`, else its parent's, else the project default for its role, and must be on that role's allow-list (`profile list`). `--model`, `--reasoning-effort`, `--permission-profile` and `--raw-agent-arg` add node flags on top; descendants inherit omitted ones from their parent. Use `--no-spawn` for a coordinator that must remain a leaf. Permission profiles validate the harness argv; they are not OS isolation. `thread start` remains a backward-compatible way to start a worker directly under the project root.

## Organizations popup, sidebar tokens and dock

**Herdr Organizations: launcher** (also the popup key `configure` binds, `prefix+a` by default) opens one popup TUI for every interactive screen: needs-you across all coordinators, projects with several coordinators nested, a New form for projects, coordinators, threads and workspaces, the project tree, a four-column threads board, thread detail, an in-popup merge confirmation and settings. The process exists only while the popup is open. It reads the state file the ticker writes and never polls Herdr.

The ticker is the one resident process. It listens to Herdr events instead of polling, reconciles once a minute, and reports display tokens that Herdr draws in its own sidebar: per coordinator workspace `●need ●working ●review` counts, per agent `$org_task`, and "needs you" in place of "blocked". `herdr-organizations sidebar install` adds the rows that render them without touching your rows or the sidebar headers. An optional dock split shows one coordinator next to its agents from the same state file.

Keybindings default to the design (`n` new, `c` coordinator, `t` thread, `b` board, `m` merge, `s` settings, `/` search, `1`-`9` jump, or answer from a thread's detail) and are editable in Settings or `~/.config/herdr-projects/tui.toml`. See [Getting started](docs/getting-started.md#the-organizations-popup).

The upstream sidebar grouping stays: `configure` writes rows that group agents and spaces by project with a bold head row and a `$hp_sub` line, and the tab bar shows `projects: N need you`. Pane metadata adds node depth, parent, role, tree order and `$org_task`. Existing overview, focus, open, inbox, routines, remote machines and reports continue to use the project root and thread records.

Root coordination is event-driven. Every turn loads the organized project digest (`context`); the ticker's `[hp inbox]` nudge waits for an idle coordinator with an empty input box. `inbox consume` prints and archives one bounded batch for clients that want it. `HANDOFF.md` carries compact objective, decisions, active work and next action across Codex or Claude replacements without copying chat transcripts or reports.

## Herdr Projects features included

Everything Herdr Projects 0.2.34 does: agent profiles and allow-lists (`profile`), tasks with owners on profiles and machines (`assignable`, `thread start --from-task`), progress self-reports from every harness (`report`, `progress`, the hooks `configure` installs), thread `read`/`keys`/`next`/`brief`/`stop`, `--kind tab|checkout`, cleanup on resolve and after a merge, `sweep`, `rename`, `set`, `open-url`, `open-file`, yolo mode and trust screens (`safety`), `doctor --fix`, `update`, the Projects popup (action **projects**, or `popup`) and the bundled `/autoproject` skill.

## Compatibility

The primary package, repository and binary are `herdr-organizations`. Cargo also builds the `herdr-projects` compatibility binary. The plugin id remains `herdr-projects`, while its visible name is **Herdr Organizations**. When replacing upstream, uninstall its registration first with `herdr plugin uninstall herdr-projects`, then build and link this checkout with `sh scripts/install.sh` and `herdr plugin link .`. The build step always compiles from source: this fork publishes no prebuilt binaries. Hooks, the tab bar command and `AGENTS.md` keep naming the `herdr-projects` binary, so entries written by upstream keep working. Existing projects remain under `~/.herdr-projects/`, and settings remain under `~/.config/herdr-projects/`. Legacy thread records load as worker nodes below `root`. Loading does not rewrite them. The CLI accepts the existing `HERDR_PROJECTS_ROOT` environment variable and configuration format.

This fork retains the upstream MIT license and attribution. See [NOTICE](NOTICE).

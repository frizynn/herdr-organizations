# Herdr Organizations

Herdr Organizations is a [Herdr](https://herdr.dev) plugin that runs a software project as a small team of coding agents. You talk to one coordinator agent. It plans the work, hands each piece to its own agent in its own pane, usually on its own git worktree, and shows you in one place what needs you.

It is for people who already run Claude Code, Codex or another agent CLI inside Herdr and want several of them working on one goal without watching every pane. It runs locally on macOS and Linux. It needs no hosted service and no API key of its own.

It is a fork of [Herdr Projects](https://github.com/eliasstravik/herdr-projects) by Elias Stravik. Everything Herdr Projects 0.2.34 does still works, with the few changes listed in [Moving from Herdr Projects 0.2.34](docs/getting-started.md#moving-from-herdr-projects-0234). This fork adds nested coordinators, scoped memory, node templates, an organization popup and a versioned JSON contract that other tools, such as [Nenu](https://github.com/frizynn/nenu), read.

## How it works

```text
you ── chat ──> root coordinator          plans and delegates, never edits code
                  │
                  ├── t-0001 worker        own worktree and branch, opens a pull request
                  ├── t-0002 coordinator   plans a subproject and starts its own children
                  │     ├── t-0003 worker
                  │     └── t-0004 worker
                  └── t-0005 worker        a tab in the project workspace, no repository

ticker (one background process)
  watches Herdr events and pull requests ──> inbox, groups, "needs you"
```

| Piece | What it is |
| --- | --- |
| Project | A folder, `~/.herdr-projects/<slug>/`. It holds the goal and settings (`PROJECT.md`), the task list (`TASKS.md`, unless the tasks live in a tracker you already use), memory, routines and a record for every thread. |
| Coordinator | An ordinary agent started in the project folder. `AGENTS.md` tells it that it is the coordinator, and the bundled skill tells it how to work. It talks with you and starts threads. It does not edit code. |
| Thread, or node | One agent with one task. A worker finishes its task and writes a report that opens with a `Status:` verdict and a short `## Summary` and ends with a `## Next` list. A coordinator node plans a larger piece and starts children under itself. A code thread works in its own git worktree on a branch named `hp/<project>/<id>-<title>`. |
| Memory | `PROJECT.md`, `MEMORY.md` and `memory/` at the root, and `nodes/<id>/` for each node. A node's brief includes the root's context and its ancestors' scopes, never its siblings' or its descendants'. `STATUS.md` is the coordinator's status file: what you asked and where each ask stands, what needs your decision and the decisions taken. What is in progress or done comes from the thread records, not from this file. You, the root or a sibling read it when you want instead of getting a report; a child coordinator keeps its own in `nodes/<id>/STATUS.md`, which its children never see. |
| Ticker | The one background process. It wakes on Herdr events, reconciles at least once a minute, reads each open pull request with `gh` at most every two minutes, copies reports home and leaves inbox items for the coordinator. |
| What needs you | Every thread is in exactly one group. In order they are waiting on you, ready for review, landing, working, idle and resolved. The popup, Herdr's sidebar, `overview` and `overview --json` show the same groups in that order. |
| Profiles and templates | A profile is a named launch setup of harness, model, effort and extra flags. A template is a saved node (role, profile name, rules and memory) to start other nodes from. |

The plugin code routes no messages and makes no decisions. The coordinator is an agent following a skill. The binary does the mechanics, one deterministic subcommand at a time. Files are the record, so a missed prompt loses nothing. [Operations](docs/operations.md#how-it-works) explains each part in detail.

## Install

You need:

- macOS or Linux, and Herdr 0.9.1 or newer. Both the client and the running server must be 0.9.1 or newer (`herdr status`).
- Rust and Cargo 1.89 or newer, and a C compiler. The plugin always builds from source. This fork publishes no prebuilt binaries.
- Git, and an agent CLI that Herdr can start, such as `claude`, `codex`, `opencode`, `gemini` or `cursor`.
- Optional. `gh`, signed in, to follow and merge pull requests. `ssh` and `rsync` for threads on other machines.

The plugin id is `herdr-projects`, so this plugin replaces an installed Herdr Projects. If you have Herdr Projects installed, follow [Moving from Herdr Projects 0.2.34](docs/getting-started.md#moving-from-herdr-projects-0234) instead. It backs up your projects and stops the old ticker first.

Otherwise, install it from GitHub:

```bash
herdr plugin install frizynn/herdr-organizations
```

Herdr's plugin marketplace does not list this repository, because the marketplace leaves out forks. Installing by name works as shown.

To work from a checkout instead, clone it to a folder that stays put and link it:

```bash
git clone https://github.com/frizynn/herdr-organizations ~/src/herdr-organizations
cd ~/src/herdr-organizations && sh scripts/install.sh && herdr plugin link .
```

The build step, `scripts/install.sh`, runs `cargo build --release --locked`. It builds `herdr-organizations` and a `herdr-projects` compatibility binary, which is the same program, and links both into `~/.local/bin`. If that folder is not on your `PATH`, add it and check the setup:

```bash
export PATH="$HOME/.local/bin:$PATH"
herdr-organizations doctor
```

Then run `configure` once:

```bash
herdr-organizations configure --dry-run   # show what it would change
herdr-organizations configure
herdr-organizations sidebar install       # optional, adds the organization counts to Herdr's sidebar
```

`configure` adds sidebar rows, a tab bar entry and the popup key `prefix+a` to Herdr's config. It installs progress hooks for Claude Code, Codex, Droid, Gemini CLI and Copilot CLI, and links the bundled `autoproject` skill for Claude Code and Codex. `herdr-organizations unconfigure` removes exactly what it added. Reload Herdr's config afterwards (`prefix+shift+r` by default).

## Your first project

1. From a shell pane inside Herdr, create a project and open its coordinator:

   ```bash
   herdr-organizations new "Billing" --goal "Ship the new billing page" --repo ~/dev/app
   herdr-organizations open billing
   ```

   `new` creates `~/.herdr-projects/billing/`. `open` starts your agent in that folder, in the pane you typed it in. You can also press `prefix+a` and then `n` to use the New form of the popup.

2. Tell the coordinator what you want, for example "Add a billing page with an API endpoint, the page itself and end-to-end tests." It restates the goal, proposes threads and waits until you name the ones to start, or say "all".

3. Watch the work. Press `prefix+a` to open the Organizations popup. Threads that wait on you come first. A thread that finishes writes a report with a `## Next` list, and the coordinator summarizes it for you.

4. Review and merge. When a thread's pull request is approved and every check passed, merge it from the popup (`m`), from your own terminal with `herdr-organizations thread merge billing t-0001`, or from Nenu. Once the pull request is merged and the thread's agent is done, the thread is resolved and its worktree and branch are removed. Its report stays in the project folder.

[Getting started](docs/getting-started.md) walks through the same steps with more detail, including trust dialogs and the sidebar.

## Daily use

Most of the time you talk to the coordinator and glance at the popup. Everything else is the same CLI that the coordinator runs.

| Surface | What you do there |
| --- | --- |
| Coordinator pane | Ask for work, answer questions, change goals, tasks, memory and settings in chat. |
| Organizations popup (`prefix+a`) | See what needs you across projects, the project tree, a board of all threads, thread detail and settings. Start projects, coordinators and threads. Merge an approved pull request. |
| Projects popup (action **projects**) | Tasks, inbox, routines, profiles, safety settings and memory, scoped to the current project. |
| Herdr's sidebar | Agents grouped by project, with lines such as `needs you · ~55%` or `review · PR #4`. |
| [Nenu](https://github.com/frizynn/nenu) | The same projects, threads and pull requests from a phone or browser. |

Commands you are likely to use yourself:

| Command | What it does |
| --- | --- |
| `herdr-organizations list` | List projects. |
| `herdr-organizations open <slug>` | Start the project's coordinator, or focus it if one is running. `--harness <kind> --model <m> --reasoning-effort <e>` chooses what it runs. |
| `herdr-organizations overview [<slug>]` | Threads grouped by what needs you. |
| `herdr-organizations node list <slug>` | The organization tree, with role, parent and live state. |
| `herdr-organizations thread show <slug> <id>` | One thread's record, group, note and Next list. |
| `herdr-organizations thread read <slug> <id>` | What a thread's pane shows now, such as a question or a permission prompt. |
| `herdr-organizations thread merge <slug> <id>` | Merge the thread's pull request, only if it is open, approved, free of conflicts and every check passed. |
| `herdr-organizations thread set <slug> <id> --auto-merge on` | Merge automatically once those same conditions hold. `--auto-fix-ci on` prompts the thread about failing checks and review comments. |
| `herdr-organizations thread pr-action <slug> <id> fix-ci` | Ask the thread to fix its pull request (`fix-ci`, `address-comments` or `resolve-conflicts`) with a message the binary writes from a fresh read; refused when the pull request does not need it. |
| `herdr-organizations thread pr-create <slug> <id>` | Open the pull request of a thread whose agent is idle and whose branch is pushed without one. |
| `herdr-organizations thread resolve <slug> <id>` | Copy the report home, then remove the worktree, the merged branch and the tab. |
| `herdr-organizations coordinator replace <slug> --profile <name>` | Stop the project's coordinator and start a new one on another profile, or with `--harness <kind> [--model <m>] [--reasoning-effort <e>]` on another harness, model and effort. |
| `herdr-organizations template save <slug> <id> --name <name>` | Save a node as a template to start others from. |
| `herdr-organizations doctor --fix` | Check the setup and repair the plugin's own files. |

The coordinator starts nodes with `node start`. You can run it yourself too:

```bash
herdr-organizations node start billing --parent root --role coordinator \
  --title "Billing API" --repo ~/dev/app --task-file - <<'TASK'
Plan the billing API. Delegate implementation tasks to children under your own node id.
TASK
```

[Operations](docs/operations.md#commands) lists every command and flag.

## Configuration

| What | Where it lives | How to change it |
| --- | --- | --- |
| Project settings (name, goal, icon, tone, repos, default profiles, `max_parallel_threads`, `auto_resolve_days`, `nudge`, `mute`) | `PROJECT.md` in the project folder | Ask the coordinator, use the Projects popup, or run `herdr-organizations set <slug> <key> <value>` |
| Agent profiles, and which ones a project may use | `~/.config/herdr-projects/config.toml` | The Projects popup, or `profile add`, `profile allow` and `profile default` at a terminal |
| Safety settings (yolo mode, who answers trust screens, whether threads start without asking, routine commands) | `~/.config/herdr-projects/config.toml` | The Projects popup, or `safety show`, `safety yolo` and `safety set` at a terminal |
| A tracker that holds the project's tasks instead of `TASKS.md`: a `list` command and an optional `link` command | `~/.config/herdr-projects/config.toml`, `[tracker."<project folder>"]` | By hand; see [Tasks from a tracker](docs/operations.md#tasks-from-a-tracker) |
| Popup keys, dock and notifications | `~/.config/herdr-projects/tui.toml` | The Settings tab of the Organizations popup |
| Sidebar rows and the popup key | `~/.config/herdr/config.toml` | `configure` and `sidebar install` |
| Projects root | `~/.herdr-projects` by default | `--root`, `HERDR_PROJECTS_ROOT`, or `config.toml` |

A profile looks like this in `config.toml`:

```toml
[profiles.deep]
agent = "codex"
model = "gpt-5.5"
effort = "high"
description = "Hard debugging and design"
```

Every Herdr agent kind is also a built-in profile of the same name, so `codex` or `claude` work with no config at all. Profiles, allow-lists and safety settings live only in `config.toml`, and the `profile` and `safety` commands that change them refuse to run without a person at a terminal.

The safety settings are soft. Agents keep the shell and permissions of their own harness. `can_spawn` and permission profiles are checks in the CLI, not a sandbox. Read [what the safety settings do and don't stop](docs/operations.md#what-the-safety-settings-do-and-dont-stop) and the [security policy](SECURITY.md) before you turn on yolo mode.

## The JSON contract, and Nenu

Other programs read Herdr Organizations through `--json` output, never through its files. Each document is one JSON object on one line that carries `"schema_version": 1`. The one exception is `template list --json`, which prints a bare array. Adding a field keeps the version. Renaming or removing a field, or changing what it means, raises it. The files inside a project folder are internal and can change without notice.

```bash
herdr-organizations overview --json
```

| Command | Prints |
| --- | --- |
| `list --json` | Projects |
| `new <name> --json` | The new project |
| `overview [<slug>] --json` | Projects, each with its threads, groups and pull requests |
| `thread list <slug> --json`, `thread show <slug> <id> --json`, `node list <slug> --json`, `node show <slug> <id> --json` | Threads or nodes |
| `inbox list <slug> --json` | Inbox items, without marking them seen |
| `thread merge <slug> <id> --json`, `thread set <slug> <id> --auto-merge on --json`, `thread pr-action <slug> <id> <action> --json`, `thread pr-create <slug> <id> --json` | The result of a pull request action |
| `template list --json`, `template show <name> --json` | Templates |
| `project show <slug> --json` | A project's settings, repos, instructions and memory |
| `task list <slug> --json` | The tasks in `TASKS.md` or the tracker, with owners and threads |
| `task lint <slug> --json` | Where the tasks disagree with the thread records |
| `project status <slug> --json` | Tasks and open threads with their state, pull requests, drift and Next lines, computed on every call |
| `routine list <slug> --json` | Routines with their schedule, command status and last and next run |

[docs/json.md](docs/json.md) defines every field.

[Nenu](https://github.com/frizynn/nenu) is a self-hosted phone and web workbench for Herdr, from the same maintainer. With Herdr Organizations installed, Nenu shows each project as its coordinator's chat with **Threads** and **PRs** tabs, and draws the organization tree the way the Organizations popup draws it. It reads `overview --json`. Actions you take in Nenu run this CLI. `node start` starts a thread or a coordinator, `coordinator replace` swaps the coordinator, `node resolve --close-view` closes a node, and `thread merge` and `thread set` act on pull requests. A merge from Nenu passes the same checks as a merge from your terminal. Nenu finds the binary on its `PATH` and honors `HERDR_PROJECTS_ROOT`. See [Nenu's README](https://github.com/frizynn/nenu#readme) to install it.

## Relation to Herdr Projects, and license

Herdr Organizations started as a fork of [Herdr Projects](https://github.com/eliasstravik/herdr-projects) and includes its 0.2.34 release. These parts come from upstream and work as they did there: projects as folders, the coordinator skill, threads in worktrees, agent profiles and allow-lists, tasks with owners, progress reports and hooks, routines, pull request follow-up, cleanup on resolve and merge, `sweep`, `rename`, safety settings and yolo mode, remote machines over SSH, the Projects popup and the `/autoproject` skill.

This fork adds, as the git history shows:

- recursive nodes, so a coordinator can start its own coordinators and workers, with instructions and memory scoped to each branch of the tree (`node start`, `node rules`)
- the versioned `--json` contract and guarded pull request actions (`thread merge`, `thread set`)
- an event-driven ticker, the Organizations popup, the optional dock and organization counts in Herdr's sidebar
- `coordinator replace`, node templates (`template`) and a History view of resolved work
- tracking that does not rely on the coordinator's prompt: `--from-task` links a task and its thread, resolving a thread whose task is still open leaves an inbox item, and `context` lists the drift between `TASKS.md`, `STATUS.md`, the thread records and your messages

For compatibility, the plugin id stays `herdr-projects`. Projects stay in `~/.herdr-projects/`, settings in `~/.config/herdr-projects/`, and `HERDR_PROJECTS_ROOT` still works. Projects written by Herdr Projects 0.2.34 load as they are, and their threads show as workers under the root. `herdr-organizations update` needs release tags, which this fork does not publish yet, so update by pulling and rebuilding ([Updating](docs/getting-started.md#updating)).

Herdr Organizations is released under the MIT license, as Herdr Projects is. [LICENSE](LICENSE) keeps the upstream copyright, and [NOTICE](NOTICE) records the fork.

## Documentation and links

| Page | What it covers |
| --- | --- |
| [Getting started](docs/getting-started.md) | Install, first project, the popup, moving from Herdr Projects, updating, removing |
| [Operations](docs/operations.md) | How it works, where files live, every command, groups, profiles, templates, safety, routines, cleanup, other machines |
| [Architecture](docs/architecture.md) | Design decisions, node records, scoped context, the popup and ticker |
| [JSON contract](docs/json.md) | Every `--json` document and field |
| [Manual validation](docs/manual-test.md) | Checks that need a real Herdr client |
| [Contributing](CONTRIBUTING.md) and [Security](SECURITY.md) | Development gates and the security model |

- Herdr: https://herdr.dev
- Herdr plugin documentation: https://herdr.dev/docs/plugins/
- Nenu: https://github.com/frizynn/nenu
- Herdr Projects, the upstream: https://github.com/eliasstravik/herdr-projects

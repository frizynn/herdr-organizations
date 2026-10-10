# Getting started: open your first organization

Install the plugin, run `configure` once, create a project, and talk to its root coordinator. Local coordinators can create nested coordinator or worker nodes; remote workers are supported, remote coordinators that may spawn are refused until a remote CLI bridge exists.

## 1. Check the prerequisites

- macOS or Linux, and [Herdr](https://herdr.dev) 0.9.1 or newer. Check with `herdr status`: both the client and the running server must be 0.9.1 or newer. After `herdr update`, a server that was already running stays on the old version until you restart it, and `herdr plugin link` or `install` then fails with `plugin_requires_newer_herdr`.
- Rust/Cargo 1.89 or newer and a C compiler: the plugin always builds from source. On macOS, `xcode-select --install` installs Apple's build tools. Install Rust with [rustup](https://rustup.rs).
- Git.
- An agent CLI Herdr can start, on `PATH`. Any of Herdr's 24 agent kinds works (`claude`, `codex`, `opencode`, `cursor`, `gemini` and more). Claude Code is the one exercised most. Every agent that can run a shell command reports its own progress: thread briefs and the coordinator skill carry the instructions. `configure` also installs hooks for Claude Code, Codex, Droid, Gemini CLI and Copilot CLI, which add a reminder about once a minute.
- Optional: `gh`, logged in, for pull request follow-up; `ssh` and `rsync` for threads on other machines.

The plugin needs no hosted service and no API key. It depends on Herdr and nothing else, no other plugin included.

## 2. Install the plugin

The plugin id stays `herdr-projects`, so Herdr Organizations replaces an installed Herdr Projects: uninstall that registration first (your projects in `~/.herdr-projects/` and settings in `~/.config/herdr-projects/` stay), then install or link this repository:

```bash
herdr plugin uninstall herdr-projects        # only when Herdr Projects is installed
herdr plugin install frizynn/herdr-organizations
# or, from a checkout:  sh scripts/install.sh && herdr plugin link .
```

Herdr runs `scripts/install.sh`, which builds `herdr-organizations` and the `herdr-projects` compatibility binary with `cargo build --release --locked`, and registers the plugin under its visible name **Herdr Organizations**. Its startup command starts a background ticker only when you have at least one project.

The install also links both binaries into `~/.local/bin` (`$XDG_BIN_HOME` when set), so `herdr-organizations` and `herdr-projects` work from a terminal; they are the same program. The plugin refreshes that link every time Herdr starts, and `herdr-projects doctor --fix` does too. It never replaces a file there, or a link to somewhere outside Herdr's plugin folder. If `~/.local/bin` is not on your `PATH`, add it in your shell profile (`doctor` says so):

```bash
export PATH="$HOME/.local/bin:$PATH"
herdr-projects doctor
```

## 3. Run configure once

```bash
herdr-projects configure --dry-run   # shows what it would change
herdr-projects configure
```

Or run `herdr plugin action invoke configure --plugin herdr-projects`. It changes four things and records each change, so `herdr-projects unconfigure` removes exactly what it added:

- **Your Herdr config** (`~/.config/herdr/config.toml`). The sub-line row under agents (`$hp_sub`), a one-line card in place of Herdr's built-in rows that shows each project's head in bold (rows you wrote yourself are left alone), the popup key `prefix+a` and a tab-bar entry `projects: N need you`. Herdr checks the result with `herdr config check` before anything is written. Pick another key with `configure --key prefix+y`; a key Herdr or you already use is refused.
- **Progress hooks** for each installed harness: Claude Code (`~/.claude/settings.json`), Codex (`~/.codex/hooks.json`), Droid (`~/.factory/settings.json`), Gemini CLI (`~/.gemini/settings.json`) and Copilot CLI (its own `~/.copilot/hooks/herdr-projects.json`). They tell an agent running in a Herdr pane how to report its progress, and remind it about once a minute. Outside Herdr they do nothing. Existing hooks and comments are kept.
- **The `autoproject` skill**, linked from the plugin's `skill/autoproject` into `~/.claude/skills` and Codex's `~/.agents/skills`. A coordinator loads it with `/autoproject` to run an independently reviewed improvement loop. A skill of that name that is not the plugin's link is left alone, and `doctor` names it. If you configured before the skill shipped, `update` links it for you.

Configure reloads the Herdr server's config. The sidebar rows are drawn by your client: if they don't show yet, run **reload config** in Herdr (`prefix+shift+r`).

If you used the standalone Agent Progress plugin, `doctor` prints the two commands that remove its hooks, so only one set runs.

## 4. Create and open a project

From a Herdr pane, run **Projects: new project** with `herdr plugin action invoke new --plugin herdr-projects`. It asks for a name and a goal, creates the project, and opens it. Or from a terminal inside Herdr:

```bash
herdr-projects new "Billing" --goal "Ship the new billing page" --repo ~/dev/app
herdr-projects open billing
```

`new` creates `~/.herdr-projects/billing/` with an `AGENTS.md` (and `CLAUDE.md` linked to it). `open` starts your agent in that folder, right in the pane you typed it in. Quit the agent and you are back at your shell. The agent reads `AGENTS.md`, which tells it that it is the coordinator and which two commands to run. Nothing is typed into it for you.

- `open billing --tab` starts it in a new tab of the project's own workspace instead. The plugin's actions and the popup always do that, and so does `open` run outside Herdr.
- When a coordinator is already running, `open` jumps to it. `open --new` starts another beside it, with a fresh conversation.
- `open billing --profile codex` starts another agent: every installed, signed-in harness is a profile, and your own profiles (a model, an effort, extra flags) are made in the popup's settings or with `profile add` ([operations](operations.md#agent-profiles)). Any agent you start by hand in that folder is a coordinator too, with no `open` needed, and several can run side by side.
- `open` resumes the agent's last session when Herdr recorded one for that profile.
- The first time, your agent may ask whether you trust the folder: answer it in the coordinator's pane.

## 5. Tell the coordinator what you want

Type in the coordinator's pane, for example: "Add a billing page: API endpoint, the page itself, and end-to-end tests."

On a new project it restates the goal, lists the repos, and asks for the first piece of work. It proposes threads and waits until you name the ones to start (or say "all"). Tell it how you like threads run ("workers use codex", "at most two at a time") and it remembers.

Everything about the project can be changed in chat: goal, instructions, repos, settings, tasks, routines, memory. You never need to edit a file.

## 6. Watch the threads

Each code thread runs in its own worktree workspace on a branch named `hp/<project>/<id>-<title>`; a task with no repository runs as a tab in the project's workspace.

- **The sidebar** shows each thread as `t-0003 · <title>` with a line under it that adds what Herdr's own state word does not say: `needs you · ~55%`, `review · PR #4`, `~40%`, `12m quiet`, `landing · PR #4`. The agent's own activity follows on the same line. The tab bar says `projects: 2 need you`. Agents and Spaces are grouped by project: each project starts with its own head row, the home Space and the coordinator, which show the project's name in bold, and its threads by need and its other Spaces follow as Herdr's own rows; agents or Spaces outside any project come last. Selecting a row lights only that row. The ticker keeps the Spaces in these blocks, so a Space you drag elsewhere moves back.
- **The popup** (`prefix+a`) lists threads, tasks, inbox, routines, settings and memory. Every thread report ends with a `## Next` list; press a number to send that line back to the thread, which then does it with its own tools. Other keys jump to a thread, stop it, restart it with another profile, resolve it, open its PR, edit settings, pause or archive the project.
- **Notifications** name the project and thread: `Billing · t-0003`, `needs you · blocked` with a sound; a new report or a merge with a softer one. `mute = true` (popup settings) silences a project.

New worktrees are folders your agent hasn't trusted yet, so a code thread usually starts with your agent's trust dialog and shows `needs you` until it is answered. Who answers is the `trust_screens` safety setting: in yolo mode the coordinator does (with `thread keys`); otherwise you do, in its pane. The brief waits until then. Codex lets a worktree inherit its repo's trust, so trusting the repo once there covers its worktrees.

When a pull request fails its checks or gets review comments, the ready-made `pr-followup` routine prompts the thread to fix them. When it merges, the thread is resolved and its worktree, workspace and branch are removed. Its report stays in `threads/<id>.md` and its files in `library/<id>/`.

## Create nodes

The root coordinator is the virtual parent `root`. A worker is a leaf. A local coordinator can create workers or more coordinators beneath itself. A local child coordinator brief gives it the project CLI prefix and tells it to use its own node id as parent. A remote coordinator cannot receive child-spawn permission because its brief cannot safely launch the local CLI from the remote machine.

Create a worker under the root:

```sh
herdr-organizations node start billing \
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
herdr-organizations node start billing \
  --parent root \
  --role coordinator \
  --title "Billing API" \
  --repo ~/dev/billing \
  --task-file - <<'TASK'
Plan the billing API. Delegate implementation tasks to children beneath your own node id.
TASK
```

`--repo` creates a worktree. Without it, the node runs in a tab in the project workspace. Add `--machine <label>` for a repository on a saved SSH machine and `--base <ref>` for a specific base branch. Use `--no-spawn` when creating a coordinator that should not create children.

A node runs `--profile` (one of the profiles the project allows, see `profile list`), else its parent's, else the project default for its role. On top of it, `--model`, `--reasoning-effort`, `--permission-profile` and repeatable `--raw-agent-arg` add node flags that its children inherit. Codex and Claude have permission adapters; raw arguments stay separate argv values, and permission or sandbox flags that conflict with a selected permission profile are refused. This is argv validation, not OS isolation.

The existing `thread start` command remains a worker-under-root alias. Use `node restart`, `node prompt`, `node list`, `node show`, `node ack` and `node resolve` for hierarchy-aware names. `node create` is an alias for `node start`.

`node resolve <project> <id>` cleans up as `thread resolve` does: it removes the worktree (never forced) and, once the pull request is merged, the local branch, and keeps the report and library. `--keep-worktree` keeps both; `--close-view` keeps them and closes the node's tab or workspace first, resolving nothing when that fails.

## The Organizations popup

Run **Herdr Organizations: launcher** from Herdr's action menu, or bind it to a key (below). Every interactive screen is one small TUI inside a Herdr popup. It exists only while the popup is open; Esc goes back, then closes it.

- **Launcher**: "needs you" across every coordinator (oldest first), then each project with its coordinators nested and their counts, then workspaces outside a project. `1`-`9` jump to a coordinator, `n` new project, `c` new coordinator, `a` turns the current workspace into a project, `/` filters, `b` opens the board, `s` settings.
- **New** (`n`): one form for a project, coordinator, thread or workspace. Up/Down switch what to create, Tab moves between fields, Left/Right choose, Enter creates. The last line shows the exact CLI call the form makes.
- **Project tree** (Enter on a project or coordinator): project › coordinators › threads. Enter goes to the selected pane, Space folds a coordinator, `t` starts a thread under it, `m` merges a reviewed pull request, `1`-`9` press that option in the pane of an agent that is waiting on you.
- **Board** (`b`): every thread in four columns (needs you, working, review, resolved), idle threads on one line and the running coordinators below. `f` cycles the coordinator filter.
- **Thread detail** (Enter on a card or a needs-you row): the question or the agent's current line, the PR and its checks, the thread's inbox items (`d` marks one done) and the last lines of its pane, read once.
- **Merge confirm** (`m`): drawn by the same process, because Herdr shows one popup at a time. It merges only when the pull request passes the same guard as `thread merge`, then resolves the thread and tells its coordinator. `k` keeps the worktree.
- **Settings** (`s`): dock side and width, resolved threads as a count or a list, which transitions show a Herdr notification, whether digits answer, every keybinding, and the ticker's health.

The popup reads `~/.herdr-projects/.organizations-state.json`, which the ticker rewrites when something changes, and redraws on a key or a change of that file. It never polls Herdr. Without a running ticker it builds the same view from the records once when it opens.

### Keybindings

Defaults follow the design: `↵` open, `n` new project, `c` new coordinator, `t` new thread, `b` board, `m` merge, `1`-`9` jump, `s` settings, `/` search, `esc` back. On a thread that needs you, `1`-`9` first opens its detail with the last lines of the pane; only there does a digit go to the agent, and only if the pane still shows those lines. Settings can turn that off. Rebind any command in Settings (select it, press Enter, press the new key; Backspace restores the default) or edit `~/.config/herdr-projects/tui.toml`:

```toml
[view]
dock = "right"        # off, right or left
dock_width = 30       # 15-50
resolved = "count"    # count or list
notify = "needs-you-and-review"  # or needs-you, off
answer = true         # 1-9 answers from the thread detail; false turns it off

[keys]
board = "w"
up = ["up", "k"]
```

Only changed keys are written. A key that two commands on the same screen would share is refused, and a bad entry in the file keeps its default and is listed under KEYS in Settings.

### Launcher key

`configure` binds `prefix+a` to `herdr-projects.open-popup`, which opens this popup. For the dock, add a key to `~/.config/herdr/config.toml` and reload Herdr's config:

```toml

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
herdr-organizations sidebar install --dry-run   # print the edited config
herdr-organizations sidebar install             # write it, keeping a dated backup
```

It appends rows to `[ui.sidebar.spaces]` and `[ui.sidebar.agents]` (and `state_text` when no row shows the state). It never edits or removes your rows and never touches the panel headers (`spaces`, `new`, `menu`, `agents`, the sort word), which Herdr draws outside `rows`. Reload Herdr's config afterwards.

## The optional dock

**Herdr Organizations: toggle dock** opens a split next to the current workspace's coordinator: needs you, review, working, idle, resolved count, inbox and a new-thread row. It is off by default; turn it on in Settings first. It reads the same state file and never polls Herdr. Enter goes to a pane and keeps the dock, `m` merges, `t` opens the New form in the popup, `?` lists the keys. The same action closes it.

## Preserve existing projects

The binary continues to read projects from `~/.herdr-projects/`, settings from `~/.config/herdr-projects/`, and `HERDR_PROJECTS_ROOT`. Projects written by Herdr Projects 0.2.34 load as they are. Legacy thread records are loaded as workers below `root` without rewriting them. Existing reports, worktrees, routines, ticker state, inbox items and remote settings remain in their current paths.

For operational details and agent permission guidance, see [Operations](operations.md). Use the [manual test guide](manual-test.md) for the keyboard, mouse and live-pane checks that require a Herdr client.

### Moving from Herdr Projects 0.2.34

Two things change for scripts: `thread list --json` and `thread show --json` print the versioned document in [json.md](json.md) instead of a bare record, and the `overview` header separates a project's goal with a colon.

Link a checkout that stays put, not a worktree you will delete or switch: every path that `configure` and `doctor --fix` write (agent hooks, the tab bar, each coordinator's `AGENTS.md`, the `~/.local/bin` links) points into the linked folder. Wait until no coordinator or thread is mid-task, then:

```bash
# 1. Back up projects and settings without the thread worktrees
#    (Herdr Projects drops the organization fields when it saves a record)
cd ~ && find .herdr-projects .config/herdr-projects -path '*/threads/t-*' -type d -prune -o -type f -print \
  | tar czf ~/herdr-projects-backup-$(date +%Y%m%d).tgz -T -
herdr-projects ticker stop
herdr plugin uninstall herdr-projects

# 2. Install from a stable checkout
git clone --branch <branch> <repository> ~/Developer/herdr/organizations-live
cd ~/Developer/herdr/organizations-live && sh scripts/install.sh && herdr plugin link .
herdr-organizations configure
herdr-organizations doctor --fix     # rewrites the old binary path in each AGENTS.md and links ~/.local/bin
herdr-organizations ticker status
```

Reload the config in Herdr, then reopen each coordinator (`herdr-organizations open <slug>`) and restart any agent that should keep reporting progress: running agents keep the hooks and paths they loaded at start. For the first minutes, watch `tail -f ~/.herdr-projects/.ticker.log` and check that open threads get no second brief.

### Going back to Herdr Projects

Herdr Projects 0.2.34 rewrites a thread record without `parent_id`, `role`, `can_spawn` and the node's model, effort, profile and agent arguments whenever it saves one, so coordinators become workers below `root`. To get the tree back when you return to Herdr Organizations, restore the thread records from the backup while no ticker runs. Its `configure` does not recognize this plugin's hooks, so remove them first:

```bash
herdr-organizations unconfigure
herdr-organizations ticker stop
herdr plugin unlink herdr-projects
rm ~/.local/bin/herdr-projects ~/.local/bin/herdr-organizations   # links into the checkout are never replaced
herdr plugin install eliasstravik/herdr-projects
herdr-projects configure
herdr-projects doctor --fix
```

## Check your setup

```bash
herdr-projects doctor          # what is installed, configured, and left over
herdr-projects doctor --fix    # repairs the plugin's own files in each project
herdr-projects ticker status
```

- **A project made with an older version**: `doctor --fix` adds `AGENTS.md`, the `CLAUDE.md` link, `uploads/` and `routines/pr-followup.md`, rewrites binary paths that point at a moved binary, and links the `autoproject` skill for each harness you configured. It never touches another plugin's entries.
- **`open` says the session is not reachable**: run it inside Herdr, or pass `--session <name>`. A project belongs to the session it was first opened in.
- **A thread stays at "no agent"**: the ticker launches agents, one per machine per tick (about 15 seconds). After three failed launches the thread is marked failed with the reason; `thread restart` tries again.
- **Herdr was restarted**: Herdr resumes Claude and Codex panes itself; the ticker gives resumed threads their names back. Threads of other agents need `thread restart`.

## Updating

`herdr-organizations update` (or `herdr-projects update`) reads the `vX.Y.Z` release tags of the plugin's `origin`, and in a linked checkout it also requires the `main` branch. This fork publishes no tags yet, so `update` and `update --check` print that and exit with status 1 without changing anything. To update, reinstall from the repository, or in a linked checkout pull and rebuild:

```bash
herdr-projects ticker stop
git pull && sh scripts/install.sh     # in a linked checkout
herdr-projects doctor --fix
herdr-projects ticker start
```

The plugin keeps your `~/.local/bin` links pointing at it. `doctor --fix` also links the `autoproject` skill for each harness you configured.

## Remove

```bash
herdr-projects unconfigure
herdr-projects ticker stop
herdr plugin uninstall herdr-projects      # or: herdr plugin unlink herdr-projects
```

A linked checkout's `~/.local/bin` links stay until you remove them (`rm ~/.local/bin/herdr-projects ~/.local/bin/herdr-organizations`); no later install replaces a link that points outside Herdr's plugin folder.

Your projects stay in `~/.herdr-projects/`; delete them yourself if you no longer want them.

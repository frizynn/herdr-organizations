# Operations and development

How Herdr Organizations works, what it writes where, how it stores recursive nodes, what its safety settings do and don't stop, and how to run nodes on other machines. Herdr Organizations is a superset of Herdr Projects 0.2.34: everything below that names `herdr-projects` works the same with `herdr-organizations`, the same program under its own name.

## How it works

- **It relies on Herdr and nothing else.** No other plugin is needed or called. Pull requests open in your browser, text files open in a new Herdr tab running `$EDITOR`.
- **A project is a folder.** `~/.herdr-projects/<slug>/` holds `AGENTS.md`, which tells any agent started in that folder that it is the coordinator and which commands to run. `CLAUDE.md` is a link to it. Several coordinators can share the folder.
- **The coordinator is an ordinary agent** following a skill (`herdr-projects skill` prints it). Local coordinator nodes may create children beneath themselves. Plugin code does not route messages, plan work or decide anything.
- **The binary does mechanics.** Starting a thread, copying reports, cleaning up after a resolve: each is one deterministic subcommand. It talks to Herdr through Herdr's CLI. The exceptions use the socket: the agent view (`focus`, `unfocus`, the default sort; Herdr 0.9.1 has no CLI for `agent.view.set`), and the ticker's `events.subscribe` connection, agent and pane lists and `org_*` token reports.
- **Agents report their own progress.** `herdr-projects report --percent N --activity "..."`, run by the agent in its pane, writes one small JSON file per pane under `<root>/.progress/` which the ticker shows on the pane's sub-line for five minutes. Thread briefs and the coordinator skill carry the instructions, so any agent reports; hooks in Claude Code, Codex, Droid, Gemini CLI and Copilot CLI (installed by `configure`) also inject them and a reminder, but only in a project's coordinator, coordinator node and local thread panes: every other pane gets no hook text and no record. The reminder follows a prompt you type, never one the ticker, the harness or another coordinator typed (`[hp inbox]`, `[hp ticker]`, `[hp routine`, `[herdr-projects ticker`, `<task-notification>`, a thread's `Read .herdr-project/...` brief line, `[from ...]`). A worker pane also gets a check-in reminder at most every five minutes while it works; coordinator panes get none. A project can turn this off: `progress = false` (see [What runs](#what-runs-and-where-records-go)). A conversation keeps what a hook injected before this change; start a fresh session to drop it. There is no daemon and no database.
- **Files are the record, prompts are nudges.** Threads write a report file, the ticker writes events to an inbox folder, and the coordinator reads state with `context` at the start of every turn. A missed prompt loses nothing.
- **One ticker per projects root** wakes on Herdr events (one `events.subscribe` connection per project socket, a 250 ms settle) and reconciles at least every minute, every 15 seconds while a launch or brief is pending; between passes it checks for briefs to send every 2 seconds. Each pass covers coordinators (any agent in a project folder, also one started by hand in a project never opened), thread state and groups, sidebar tokens and the per-project grouping of agents and Spaces, pending prompts, changed reports, pull requests (every two minutes; once an hour, those of resolved threads whose record still says `OPEN`), routines, auto-resolve, notifications. Remote machines are polled once a minute.
- **Tools are found even under a bare `PATH`.** The binary appends `/opt/homebrew/bin`, `/usr/local/bin`, `~/.local/bin` and `~/.cargo/bin` to its own `PATH`, so a ticker started by Herdr finds `gh` and `rsync`.
- **Cleanup is part of the flow, never forced.** Resolving a thread removes its worktree (Herdr and git refuse a dirty one, and the plugin never forces) and, once its pull request is merged, its local branch. Reports and library files always stay. Text from reports, pull requests and command output is never placed in a prompt.

## Where things live

```
~/.herdr-projects/<project>/
  PROJECT.md              settings (TOML between +++ lines) and your standing instructions
  AGENTS.md, CLAUDE.md    who is the coordinator, by working directory; written by the binary
  STATUS.md               the coordinator's status file: asks, pending decisions, decisions
  MEMORY.md, memory/      project memory; the coordinator's
  nodes/<id>/             a node's INSTRUCTIONS.md, MEMORY.md and memory/*.md; a coordinator node's own STATUS.md
  templates/<name>/       the project's node templates: TEMPLATE.toml, RULES.md, MEMORY.md, memory/
  TASKS.md                the task list, each task with optional indented notes; the coordinator's
                          (absent and ignored when a tracker holds the tasks)
  routines/<name>.md      routines, including pr-followup.md; the coordinator's
  uploads/                files you give the threads
  scratch/                the coordinator's temporary files
  threads/<id>.toml       thread record          threads/<id>.md       home copy of its report
  threads/<id>.task.md    the task and every forwarded prompt (## Follow-ups)
  threads/<id>.next.md    Next lines the coordinator added    threads/<id>/  a tab thread's folder
  inbox/, inbox/done/     events for the coordinator
  library/<id>/           home copy of files a thread produced
  .state/                 status, coordinator record, live coordinators, ticker state, merge gate verdicts, lock,
                          the tracker's last task list (tracker.json)
~/.herdr-projects/.ticker.lock  .ticker.log  .progress/  .trash/
~/.herdr-projects/.organizations-state.json      the popup's view model, written by the ticker
~/.config/herdr-projects/tui.toml                popup and dock keybindings and view settings
~/.config/herdr-projects/config.toml             yours: root, profiles, safety and tracker tables, machines
~/.config/herdr-projects/templates/<name>/       global node templates, laid out as a project's
~/.config/herdr-projects/owned.json              what `configure` changed, for `unconfigure`
~/.config/herdr-projects/approved-routines.json  written only by `routine approve`
```

With `records` set in `PROJECT.md`, the records a person reads later go to that folder instead, in the same layout: `threads/<id>.md`, `threads/<id>.task.md`, `threads/<id>.next.md`, `library/<id>/`, the root's `STATUS.md` and `nodes/<id>/STATUS.md`. Everything else in the tree above stays in the project folder (see [What runs](#what-runs-and-where-records-go)).

Every thread works from `<its working directory>/.herdr-project/<project>-<id>/`: `brief.md` (written by the binary), `report.md` and `library/` (written by the agent). In a git repository that folder is in `info/exclude`, so nothing in it is committed. Git therefore treats it as clean and removing a worktree deletes it, which is why a resolve keeps the worktree when the final copy home was partial.

`PROJECT.md` settings, all changeable from the Projects popup's settings section, from chat, or with `herdr-projects set <project> <key> <value>`: `name` (the workspace label), `goal`, `icon` and `tone` (the glyph and colour a client such as Nenu draws for the project; empty, the default, lets the client pick; the popup does not list them), `repos` (`repos.add PATH[@MACHINE]`, `repos.remove PATH`), `coordinator_profile` and `thread_profile` (the default [profiles](#agent-profiles); the old names `coordinator_agent` and `thread_agent` still read), `max_parallel_threads` (10), `auto_resolve_days` (7), `nudge` (`true`), `mute` (`false`), `progress` (`true`), `turn_reminder` (`true`), `records` (empty: the project folder). [What runs](#what-runs-and-where-records-go) explains the last four.

The virtual `root` is the project coordinator. A thread record is a node: a new worker has `parent_id = "root"`, role `worker` and cannot spawn. A local coordinator node can create worker or coordinator children at any depth when `can_spawn = true`. Records written before nodes load as workers under `root`; reading never rewrites them. A brief includes the project context, then each ancestor's node scope from root to parent, then the node's own; siblings and descendants are excluded. A coordinator node that may spawn gets a coordinator brief instead of the thread brief, plus its own `nodes/<id>/STATUS.md` (at most 4,000 characters, in the records folder when `records` is set), which no other brief includes; `node show` prints that path. See [Architecture](architecture.md) for validation and storage details.

## Commands

| Command | What it does |
| --- | --- |
| `new <name> [--goal] [--repo PATH[@MACHINE]]... [--thread-profile P] [--coordinator-profile P]` | Create a project folder. The profiles default to `[defaults]` in config.toml, else `claude`. A project with a [tracker](#tasks-from-a-tracker) gets no `TASKS.md`. |
| `open <project> [--profile P \| --harness KIND] [--model M] [--reasoning-effort E] [--new] [--tab] [--session N \| --socket P] [--rebind]` | A coordinator agent in the project folder; focuses a running one. From a shell pane inside Herdr it runs in that pane and quitting it returns to the shell; `--tab`, the popup, actions and a terminal outside Herdr use a tab of the project's workspace. `--harness`, `--model` and `--reasoning-effort` are the [node flags](#node-flags) of the same name, checked against the coordinator allow-list before any pane is made. |
| `context <project> [--peek]` | The digest the coordinator reads every turn, with its [drift](#drift) section. |
| `assignable <project> [--refresh] [--check OWNER]` | Who TASKS.md tasks may be assigned to (see [Task owners](#task-owners)); `--check` refuses an owner that is not valid. |
| `coordinator prompt <project> --text-file F` | A sentence to the coordinator (the popup's task keys use it). |
| `coordinator replace <project> --profile P \| --harness KIND [--model M] [--reasoning-effort E]` | Stop the project's running coordinator and start a new one on profile `P`, or on harness `KIND`, with the model and effort given. The choice is checked before anything stops. The popup's `x` on the project coordinator does the same. |
| `thread start <project> --title T [--repo PATH] [--kind worktree\|tab\|checkout] [--profile P] [--machine M] [--base REF] --task-file F [--from-task TITLE]` | New thread; `-` reads the task from standard input. `--kind` is the placement; `--profile` is the agent, one the project allows. `--from-task` adds a TASKS.md task's notes, takes `--profile` and `--machine` from its owner and appends the thread's id to the task's line; with a [tracker](#tasks-from-a-tracker) it takes the task's id or title and tells the tracker instead. |
| `thread prompt`, `thread next [--line N \| --add TEXT]`, `thread stop`, `thread restart [--profile P]` | Steer a thread. Prompts are recorded in its task file. |
| `thread read [--lines N]`, `thread keys [KEY]... [--text T]`, `thread brief` | Answer a thread's pane without going there: print what it shows (a trust dialog, a question menu, a permission prompt), then type text and press keys (`up`, `down`, `enter`, `esc`, `tab`, a digit). `thread keys` refuses a trust screen when `trust_screens` is `user`. `thread brief` sends a brief the ticker has not delivered yet (the ticker waits for a settled, empty input box, counts a brief sent only once the agent starts working, never types it twice, and after three unconfirmed tries leaves an inbox item). |
| `thread list/show [--json]`, `thread ack`, `thread adopt` | Look at threads. `--json` prints the versioned document of [docs/json.md](json.md). |
| `thread resolve [--keep-worktree \| --close-view] [--discard-uncopied] [--skip-copy] [--reopen]` | Final copy home, then clean up. `--close-view` keeps the worktree and branch, closes the thread's tab or workspace first, and resolves nothing when it cannot close it. |
| `thread merge [--method squash\|merge\|rebase] [--json]`, `thread set [--auto-fix-ci on\|off] [--auto-merge on\|off] [--json]` | Merge a thread's pull request only when it is open, approved, conflict-free and every check passed (with a [merge gate](#merge-gate), when the gate passes instead of the approval), pinned to the checked head commit; and the per-thread pull request automation. `--method` defaults to the project's `merge_method`, else squash. Both refuse from an agent pane of the project. Auto-fix leaves events an enabled `pr` routine covers to that routine. |
| `thread pr-action <project> <id> fix-ci\|address-comments\|resolve-conflicts [--json]` | Ask the thread to fix its pull request: a follow-up with counts and `gh` commands the binary wrote from a fresh read, refused when the pull request's state does not call for it, and not sent again while nothing changed since it, auto-fix or a `pr` routine asked (see [Pull request actions](json.md#pull-request-actions)). |
| `thread pr-create <project> <id> [--json]` | Open the pull request of a local worktree thread whose agent is idle and whose branch origin has without one: its title, its report's `## Report` as the description, into its base. It is recorded at once. |
| `node start <project> --parent root\|<id> [--role worker\|coordinator] [--template NAME] --title T [--can-spawn \| --no-spawn] [--profile P \| --harness KIND] [--model M] [--reasoning-effort E] [--permission-profile P] [--raw-agent-arg A]... [--repo PATH] [--kind K] [--machine M] [--base REF] [--rules-file F] --task-file F [--from-task TITLE]` | A node in the tree (`node create` is an alias); `--role` defaults to the template's, else `worker`. `--from-task` works as for `thread start`. `thread start` is the same for a worker under `root`. |
| `template save <project> <id> --name N [--global] [--description D] [--force]`, `template list [--project P] [--json]`, `template show <name> [--project P] [--json]`, `template delete <name> [--project P]` | [Templates](#templates). |
| `node restart [--profile P]`, `node prompt`, `node list [--json]`, `node show [--json]`, `node ack`, `node resolve [--keep-worktree \| --close-view]` | Hierarchy-aware lifecycle; `node list` prints the tree in stable preorder. |
| `node rules <project> <id> [--text-file F]` | Print a node's standing instructions, or replace them. The node and its descendants read them on their next brief or restart. |
| `inbox list [--json]`, `inbox consume`, `inbox done` | Read every item without marking seen; print and archive one bounded batch (at most 32) of the coordinator's items after it was delivered; mark any item handled. |
| `list [--json]`, `new ... --json`, `overview [project] [--json]`, `project show <project> [--json]` | The project documents of [docs/json.md](json.md). |
| `task list <project> [--json]` | The tasks in `TASKS.md`, or the [tracker's](#tasks-from-a-tracker) with each id in a last column. Only the coordinator writes the file; ask it, or send it a sentence with `coordinator prompt`. Fails while the tracker is unavailable. |
| `task lint <project> [--json]` | Every problem the [drift](#drift) section draws from in the tasks, without its 10-line cap. It changes nothing. |
| `project status <project> [--json]` | The project's tasks and open threads with their state, pull requests, drift and Next lines, computed from the files on every call (see [docs/json.md](json.md#project-status)). |
| `sidebar install [--config PATH] [--dry-run]` | Append the rows that show the organization tokens (`$org_need`, `$org_work`, `$org_review`, `$org_task`) to Herdr's config, keeping your rows. |
| `sweep <project> [--dry-run] [--yes]` | Remove what nothing uses any more. |
| `set <project> <key> <value>`, `routine list [--json]/toggle/approve`, `safety show/yolo/set` | Settings, routines and safety (see below). |
| `profile list [--project P [--json] \| --names]`, `profile resolve [NAME]`, `profile add/edit/remove`, `profile allow threads\|coordinator NAMES... [--project P] [--all]`, `profile default threads\|coordinator NAME` | [Agent profiles](#agent-profiles). Everything but `list` and `resolve` needs a person at a terminal. |
| `pause`, `resume`, `archive`, `unarchive`, `delete [--force]` | Project lifecycle. |
| `rename <project> <new-slug> [--name NAME] [--dry-run]` | A new slug (folder name), and with `--name` a new display name. Refused while a thread is not resolved. While agents run in the project folder (its coordinator, which may run it itself) the ticker takes over: once they are idle (at most 5 minutes) it closes their panes, renames, reopens the coordinator in the new folder resuming its conversation, and prompts it with a note; `thread start` is refused meanwhile. The folder moves in one step; the thread records, the coordinator record, `AGENTS.md`, the project's `[safety]` table (yolo, profile lists), routine approvals and the home Space's label follow it. Branches and worktrees keep `hp/<old>/` and `sweep` still finds them; the coordinator's conversation follows (Claude Code's transcript is copied to the new folder; other harnesses resume by id, one that cannot starts anew). It lists what it could not update (other machines), then runs `doctor`. Run it again to finish one that stopped halfway. |
| `popup [project]`, `focus [project]`, `unfocus`, `overview [project] [--wait]`, `needs-you --line` | Views. `popup` and the **projects** action open the Projects popup below; the **Herdr Organizations** launcher and the popup key open the Organizations popup. |
| `configure [--key K] [--hooks-only] [--dry-run]`, `unconfigure`, `report`, `progress` | Sidebar, keys, hooks, the `autoproject` skill, self-reports. |
| `open-file <path>`, `open-url <url>` | Open a text file in a new tab with `$EDITOR`, or a PR in the browser. |
| `ticker start \| run \| stop \| status`, `doctor [--fix]`, `skill` | Housekeeping. `doctor --fix` also relinks `~/.local/bin/herdr-projects`. |
| `update [--check]` | Update to the newest `vX.Y.Z` tag on origin: fetch, rebuild, `doctor --fix`, restart the ticker. This fork publishes no tags yet, so it exits 1 ([Updating](getting-started.md#updating)). |

`coordinator prompt`, `thread prompt` and `node prompt` sign what they send from a pane the project recorded for its coordinator or one of its local nodes: the text starts with `[from the root coordinator]` or `[from <id> "<title>"]` (the title on one line, at most 60 characters), so the receiver can tell another coordinator from you. That is the text typed and recorded in the task file. From your own terminal or any other pane, the text goes as typed.

## Groups

Every thread is in one group, shown in the sidebar, the popup and the digest, needs-you first:

1. **Waiting on you** (`needs you`): a failed start, a pane that is gone before a report, a launch stuck on a dialog, the agent blocked on a question or permission for 30 seconds, or the agent's own report `Waiting for you` while it is not working.
2. **Ready for review** (`review`): a report you haven't acknowledged, or a report with an open pull request, while the agent is not working.
3. **Landing**: an open pull request that is approved.
4. **Working**: the agent works, a launch is under way, or the agent reported progress under 100% in the last five minutes.
5. **Idle**, then **Resolved**.

Threads idle for `auto_resolve_days` are resolved (and cleaned) after a final copy home.

## The Organizations popup and dock

The popup key (`prefix+a`, the **open-popup** action) and the **Herdr Organizations: launcher** action open the Organizations TUI: needs-you across all coordinators, projects with their coordinators, a New form for projects, coordinators, threads and workspaces, the project tree, a four-column board, thread detail with digit answers, an in-popup merge confirmation and settings. It reads `.organizations-state.json` and calls Herdr only for an action you ask for. The **toggle dock** action opens the same view in a split for one coordinator. Keybindings and view settings (dock, resolved rows, which transitions notify) live in `tui.toml` and in its Settings screen.

## The Projects popup

The **projects** action (or `popup`) opens it, scoped to the current workspace's project: the coordinator's workspace or a thread's, found by where its panes work. From any section, `P` opens a project picker with All projects first and the current scope highlighted: `↑`/`↓` (or `k`/`j`) move, `↵` switches, `esc` closes (archived projects are skipped; `↵` on a settings project row still jumps there). In the picker `/` filters by name or slug as you type; `esc` clears the filter, then closes. `/` in any section opens the picker straight into the filter. Outside a project it opens on all projects. Every key runs a CLI command; the popup can do nothing the CLI cannot.

| Section | Keys |
| --- | --- |
| threads | `↵` jump to the pane · `1`-`9` send that Next line to the thread · `s` stop (Escape) · `r` restart with a kind picker · `a` ack · `x` resolve · `o` open the PR · `i` detail (report, Next list, files: `↵` opens, `y` copies the path) · `c` start or focus a coordinator of a chosen kind · `S` sweep |
| tasks | `↵` jump to the delegated thread (or show the notes of a task without one) · `i` notes (a task with notes ends in `≡`) · `d` delegate · `m` done · `D` drop (each sends a sentence to the coordinator, which stays the only writer of TASKS.md, or changes a tracker's task with the tracker's own tool). A tracker's tasks show from its last list, with their ids; the popup never runs `list` itself. |
| inbox | `↵` detail · `a` done |
| routines | `↵` enable or disable · `i` the prompt |
| settings | `↵` edit (also a safety row) · `Y` yolo mode on or off (asks before turning on) · `p` pause or resume · `A` archive · `X` delete (asks first). Unscoped, the rows are the all-projects safety defaults |
| memory | `↵` read (change memory by asking the coordinator) |

## Node flags

A node's profile is `--profile` (or `--harness KIND`, the profile of that name: yours when you defined one, else the built-in), else its parent node's, else the project's default for its role: `thread_profile` for a worker, `coordinator_profile` for a coordinator node. It is checked against that role's allow-list at creation and at every launch. On top of the profile a node can carry `--model`, `--reasoning-effort` (the same flags as a profile's), `--permission-profile` and repeatable `--raw-agent-arg`; children inherit them, raw arguments only while the harness stays the same, and `thread restart --profile` clears them. A node's model and effort replace its profile's. Codex permission profiles are `read-only`, `workspace-write` and `full-access`; Claude Code's are `default`, `plan`, `accept-edits` and `bypass-permissions`; other harnesses take raw arguments only. Conflicting permission, sandbox or approval flags in raw, profile or project arguments are refused before placement, and a node with its own permission profile gets no yolo flags. These flags validate argv; they are not OS isolation.

`open` and `coordinator replace` take `--harness`, `--model` and `--reasoning-effort` with the same meaning and the same checks, for the project's coordinator: `--harness KIND` is the profile of that name and must be on the coordinator allow-list, and the model and effort replace the profile's. The coordinator's record (`.state/coordinator.json`) keeps the profile, harness, model and effort it started with, and `profile list --project P --json` reports them as `last` so a client can offer the same choice again. A choice given to `open` focuses or resumes only a coordinator that runs that same profile, model and effort (a record from before they were kept counts as the profile's own); another choice starts a fresh one. A plain `open` keeps the old rule: it focuses any running coordinator and resumes the default profile's session, on that profile's own model and effort. Permission profiles and raw arguments are not taken there: the coordinator's belong in a profile.

## Templates

A template is a node saved to start others from: its role, spawn permission, profile name, rules (`RULES.md`, from its `INSTRUCTIONS.md`) and memory (`MEMORY.md` and `memory/*.md`). `template save <project> <id> --name N` writes it under the project, and `--global` under `~/.config/herdr-projects/templates/` for every project. Saving over an existing one needs `--force`, which keeps its creation time. Memory that a brief would omit (over 8 KiB per file, 32,000 bytes or 64 files) is refused, so a copy never drops a file silently.

`node start --template N` looks in the project first, then the global templates. It gives the node what the command left unset: the role unless `--role` is given, the spawn permission while the role is the template's, the profile unless `--profile` or `--harness` is given, and the rules unless a non-empty `--rules-file` is given. The memory is always copied into the new node's scope, where it reaches the node and its descendants like any node memory. Later changes to the template or to the node it came from do not reach nodes already started; save again to refresh it. The profile is checked against the role's allow-list as any `--profile` is.

A template keeps the profile by name only. Node flags (`--model`, `--reasoning-effort`, `--permission-profile`, `--raw-agent-arg`) are not saved: a launch setup worth reusing belongs in a [profile](#agent-profiles). `template list --project P` shows the project's own over global ones of the same name, and `template delete` removes exactly the scope it names (the project's with `--project`, else the global one). When the project has templates of their role, the popup's New coordinator and New thread forms show a Template choice with them.

## Safety settings

They live in `~/.config/herdr-projects/config.toml`, per project and as an all-projects default; a project's own value wins, then the default, then the built-in value. Change them in the Projects popup's settings section (`Y` toggles yolo mode, `↵` edits a row) or in a terminal; `safety show <project>|--global` prints them and where each comes from.

```sh
herdr-projects safety yolo billing on          # or off, or default (use the all-projects value)
herdr-projects safety yolo --global on         # every project without its own value
herdr-projects safety set billing routine_commands on
herdr-projects safety set billing start_threads default
```

`safety yolo` and `safety set` refuse unless standard input and output are a terminal, and ask `y/N` before writing. What they write:

```toml
[safety.default]                   # all projects
yolo = true

[safety."/Users/you/.herdr-projects/billing"]
yolo = false                       # on: threads start without asking, agents skip permission prompts
start_threads = "propose"          # or "auto": the coordinator starts threads without asking
trust_screens = "user"             # or "coordinator": who answers trust screens in thread panes (unset: follows yolo)
coordinator_agent_args = []        # extra arguments for every coordinator's agent CLI
thread_agent_args = []             # extra arguments for every thread's agent CLI
routine_commands = false           # true lets approved routines run shell commands
merge_gate = ["/Users/you/bin/approve.sh"]  # your command decides merges instead of GitHub's approval (by hand only)
merge_gate_timeout = 120           # seconds (1 to 3600) before the gate counts as a refusal
merge_method = "squash"            # or "merge" or "rebase": the popup, auto-merge and thread merge without --method
auto_merge = false                 # true: the ticker auto-merges every thread of the project
```

**Yolo mode** is the one switch for "never stop to ask": `start_threads` becomes `auto` whatever it says, and every coordinator and thread launches with its own harness's skip-permissions flag, added after the profile's own arguments: Claude Code `--dangerously-skip-permissions`, Codex `--dangerously-bypass-approvals-and-sandbox`, Gemini CLI and Qwen Code `--yolo`, Cursor Agent `--force`, OpenCode `--auto`, Copilot CLI `--allow-all-tools`, Amp `--dangerously-allow-all`, Pi nothing (it never asks). Other kinds have no known flag: their agents still ask (`safety show` says so for the project's kinds). Routine commands are not part of yolo mode: a routine command runs with no agent in the loop, so it stays behind `routine_commands` and a per-command approval.

**Trust screens** are an agent's "trust this folder?" dialog, its restricted-folder chooser, and its hooks or settings review (Codex's "Hooks need review"). The harness saves the answer for every later session in that folder, so `trust_screens` says who gives it: `coordinator` (it answers them with `thread keys` for the thread's own folder) or `user` (it tells you, and `thread keys` refuses while one shows). Unset, it follows yolo mode: on, the coordinator; off, you. Whatever the setting, nothing is typed into a trust screen: a thread's brief waits, and `thread prompt` refuses, while one shows, even when Herdr reads the pane as idle, and the thread shows `needs you`.

A change reaches agents launched after it. Running agents keep the flags they started with until restarted: `r` on a thread in the Projects popup, or quit the coordinator and `open` it again (it resumes its session). A running coordinator sees a new `start_threads` at its next `context`.

`thread_agent_args` and `coordinator_agent_args` are from before profiles. They were written for the harness of the project's default profile, so they now reach only a built-in profile of that harness: Claude flags stay with Claude threads and no longer break a Codex thread. Move them into a profile of your own (below) and remove them.

### Merge gate

`merge_gate`, `merge_gate_timeout`, `merge_method` and `auto_merge` are edited by hand in `config.toml`, in the project's table or in `[safety.default]`. `safety set` refuses them, `default` included, and nothing in the popup changes them, so an agent cannot turn merging on through the CLI. The gate's program is an absolute path or a bare name looked up on `PATH`; a relative path is a config error, because it would resolve in the thread's repository or the project folder, which agents write. A `config.toml` that exists but cannot be read is an error too, not an empty file, so a merge fails instead of skipping the gate.

A binary from before these keys ignores them: its `thread merge`, popup and ticker merge under GitHub's rules and never run the gate. Update every installed binary before a project relies on the gate. `context` and `project show --json` name the gate's program (not its arguments), the method and `auto_merge`.

Without `merge_gate`, `thread merge` needs an approving review and at least one reported check, as before. With it, the command decides those two instead. What GitHub reports still blocks in every case: a closed pull request, a draft, a merge conflict, a check that failed or has not finished, a head branch, repository or base that is not the thread's, an unknown head commit. The gate never runs on such a pull request.

The gate runs only when something merges (`thread merge`, the popup's merge, auto-merge), just before `gh pr merge`, on the head commit the merge is then pinned to with `--match-head-commit`: a push while the gate runs makes GitHub refuse the merge. It is an argv list, run without a shell and without standard input, in the thread's local repository (the project folder for a remote or repository-less thread), with these variables:

| Variable | Value |
| --- | --- |
| `HP_PROJECT`, `HP_THREAD_ID` | The project slug and the thread id. |
| `HP_PR_URL`, `HP_PR_NUMBER`, `HP_REPO` | The pull request, its number and its `owner/name`. |
| `HP_HEAD_SHA`, `HP_HEAD_REF`, `HP_BASE_REF` | The head commit it decides on, the thread's branch and the base branch. |
| `HP_WORKTREE` | The thread's worktree, when it exists on this machine. |

Exit 0 passes. Any other exit, running longer than `merge_gate_timeout` seconds (120 by default) or a command that cannot start refuses. The reason is the first line of its standard output, without control characters and cut to 200 characters, or `merge gate exited <code>` and `merge gate timed out`. It goes to `thread merge`'s error, one `pr` inbox item each time the reason changes (`merge gate refused`), the `merge_blocker` of `--json` and the popup. It never goes into a prompt. The last verdict per thread and head commit is kept in `.state/merge-gate.json`, so those read it without running the command; before the gate has run on a commit, they show nothing blocking, and a merge runs it. A refusal does not stop the next merge: `thread merge`, the popup's merge and Nenu ask the gate again, since its answer for the same commit can change.

`auto_merge = true` makes the ticker auto-merge every thread of the project, as `thread set --auto-merge on` does for one; a thread cannot opt out. With a gate, a refusal on a head commit is asked again at every pull request check (about every two minutes) until it passes or a new commit arrives, so a reviewer's verdict that lands later for the same commit is picked up.

## Agent profiles

A profile is a named launch setup: a harness (any Herdr agent kind), a model, a reasoning effort, extra arguments and a one-line description. The coordinator only ever chooses a profile by name (`thread start --profile deep`, `open --profile claude`), and `context` lists the ones it may use with their descriptions, so it can pick one per task. `thread start` takes no launch flags (`--agent-arg` is gone). `node start` accepts the [node flags](#node-flags) on top of an allowed profile, and refuses permission or sandbox flags that conflict with a selected permission profile. Yolo mode still adds the harness's skip-permissions flag on top of a profile's arguments.

```toml
[profiles.luna]
agent = "omp"
args = ["--config", "~/.omp/agent/luna.yml"]   # `~/` is expanded for local agents
description = "Cheap tier for small, clear tasks"

[profiles.deep]
agent = "codex"
model = "gpt-5.5"
effort = "high"
description = "Hard debugging and design"

[profiles.claude]                  # replaces the built-in `claude`
agent = "claude"
args = ["--add-dir", "~/dev/shared"]

[defaults]                         # what `new` writes into a new PROJECT.md
thread_profile = "claude"
coordinator_profile = "claude"

[safety.default]                   # every project without its own list
thread_profiles = ["claude", "luna", "deep"]   # absent: every profile
coordinator_profiles = ["claude"]

[safety."/Users/you/.herdr-projects/billing"]
thread_profiles = ["luna"]         # this project's own list wins
```

- **Built-ins.** Every Herdr agent kind is a profile of the same name with no arguments, so `thread_profile = "codex"` works with no config at all. Lists show the built-ins whose CLI is on `PATH` and looks signed in. Herdr exposes neither (its `integration.list` sees only hook files and some binaries), so the binary checks itself, offline: credential files and variables such as `~/.codex/auth.json`, `~/.claude.json`'s account, `~/.gemini/oauth_creds.json`, `CURSOR_API_KEY`. A kind with no known check counts once installed. Any built-in can be named even when it is not listed (a remote machine's, say).
- **Model and effort** become the harness's own flags: `--model NAME` for every harness; effort as `--effort` (Claude Code: low, medium, high, xhigh, max; Copilot CLI: none to max), `-c model_reasoning_effort="…"` (Codex: none to ultra), `--thinking` (pi, oh-my-pi: off to max). Cursor puts effort in the model id (`gpt-5.6-sol-xhigh`), and Gemini CLI and OpenCode have no launch flag for it: use the model id or `args`.
- **Codex runs in its pane's own process.** Every Codex coordinator, thread and node starts with `--no-daemon`, a resume too, because Herdr's session hook has to run in the pane's process. On Codex's shared background server the hook runs in that server and reports to the pane that started it, so the pane never learns its conversation. A profile that already passes `--no-daemon`, or `--remote` (a server you chose), is left as written. Codex 0.156 or later knows the flag.
- **Allow-lists.** A project's own `thread_profiles` / `coordinator_profiles` wins, then `[safety.default]`'s, then every profile. A profile off the list is refused at `thread start`, `thread restart` and `open`, and the ticker checks again at every launch: a thread whose profile you removed or disallowed since fails with the reason and an inbox item. The project's defaults (`thread_profile`, `coordinator_profile` in PROJECT.md) must be allowed too; `doctor` says when one is not.
- **Who changes what.** Profiles and lists live only in config.toml, which the coordinator never writes. `profile add/edit/remove/allow/default` refuse without a person at a terminal, as `routine approve` does; the Projects popup's settings section writes them directly (`n` new profile, `↵` edit, `d` delete, `↵` on a list toggles profiles with space). The default profiles in PROJECT.md are ordinary settings the coordinator may change when you ask, only to an allowed profile. This is a soft boundary: an agent that fakes a terminal or edits config.toml by hand is stopped only by its own permission prompts.
- **Old threads and coordinators.** A thread started before profiles keeps its harness and its stored model flag. `open` resumes or reuses a coordinator only when it runs the same profile (and, when they were given, the same model and effort: see [node flags](#node-flags)); one started before profiles counts as the built-in of its kind.
- **A lean coordinator, optional.** A coordinator node's brief already tells it to hand every hands-on task to a child. To also take Claude Code's edit tools away from it, give it a profile with raw arguments, allowed for coordinators only:

  ```toml
  [profiles.lean-coordinator]
  agent = "claude"
  args = ["--disallowedTools", "Edit,Write,NotebookEdit"]
  description = "Coordinator node without edit tools"

  [safety.default]
  coordinator_profiles = ["claude", "lean-coordinator"]
  thread_profiles = ["claude"]
  ```

  Start the node with `--profile lean-coordinator`. Its children get its profile unless they name one, so keep it off `thread_profiles`: a worker started without `--profile` is then refused instead of starting without edit tools. Use a profile rather than `--raw-agent-arg`, because a node's raw arguments pass to every child on the same harness and one child cannot drop them. This changes argv only and is not a security boundary: the agent keeps its shell and can still write files through it, which is also how it updates its own `STATUS.md`. `--disallowedTools` is Claude Code's flag (2.1.301); other harnesses need their own.
- **Choices for a client.** `profile list --project P --json` prints, for the coordinator and for threads, the harnesses that role may run (installed, signed in and allowed), each with its effort levels and default model, every allowed profile with its model and effort, the project's default, and the coordinator's last choice ([docs/json.md](json.md#profile-choices)).

## Task owners

Each task in `TASKS.md` has at most one owner, in brackets after its title. The thread running it comes after, as status:

```
- [ ] Write the release notes (me)
- [ ] Rename the settings keys (codex-fast)          a profile, on this machine
- [ ] Fix the M1 build (@elias-macbook-pro-m1)       that machine; its coordinator picks the profile
- [ ] Profile the ticker (deep@elias-macbook-pro-m1) that profile on that machine
- [ ] Look into Safari logouts                       unassigned
- [ ] Review the contract (Priya)                    a person, only when you name them
- [ ] Fix login (claude) · t-0007                    delegated: t-0007 runs it
```

- **Agent owners.** The coordinator assigns only valid names: the thread profiles the project allows here, and each machine with the profiles it lists. Machines come from two places, used together: `herdr machine list` (their profiles are fetched over SSH by running `herdr-projects profile list --names` there), and config.toml, for a machine this one cannot reach, such as a sandboxed VM that only learns the names:

  ```toml
  [machines.elias-macbook-pro-m1]
  profiles = ["claude", "codex-fast"]   # names only; gives no access
  ```

  A machine with an `ssh` key there is fetched as well. A machine is valid only if one of these sources knows it, and `profile@machine` only if that profile is in the machine's list from the lookup or from config.toml. `context` prints one `Assignable:` line (`claude, codex-fast, @m1: claude|pi`), and `assignable <project>` the full list. The SSH lookups are cached for an hour in `<root>/.machines.json`, so `context` does not reach other machines every turn; `assignable --refresh` looks again. A machine that did not answer and has no profiles in config.toml shows as `@m1 (not reached, profiles unknown)`: `@m1` is valid, `profile@m1` is refused. With no machines, only local profiles are valid.
- **Checked owners.** The coordinator checks a profile or machine owner with `assignable --check` before writing it and refuses one that is not valid.
- **People.** `me`, or any name or text you give (`(Elias)`, `(Priya Rao)`), is written only when you name that owner. A bare name that is no profile here is a person: shown as written and never delegated.
- **Delegating.** `thread start --from-task "<title>"` (and `node start --from-task`) starts the thread with the owner's profile and machine (`--profile`, `--machine`); a flag that contradicts the owner is refused. A remote thread runs its machine's own definition of the profile (see [Threads on other machines](#threads-on-other-machines)), and a `@machine` owner gets that machine's default thread profile. The binary then appends ` · t-0007` to the end of the task's line, replacing a thread id that was there; the other lines are left as they were. It reads `TASKS.md` again right before writing it, under the project's lock, which keeps two delegations apart; the coordinator does not take that lock, so an edit it saves in that same instant can be lost. A title that no task or two tasks have is refused before anything is created. If the start fails later, the line keeps the id of the failed thread, which `thread restart` retries. The thread's record keeps the title (`task` in [`--json`](json.md#thread)), and a template's profile counts only when neither a flag nor the owner names one.
- **Resolving.** When a thread is resolved by any path (by hand, on merge, or automatically) while `TASKS.md` still has an open task on it, the inbox gets one `task` item: `PR merged, task open` or `thread resolved, task open`. With a tracker, the same item comes when its list still has the task open, after `link` has heard `merged` or `resolved`.
- **Old lines.** `(agent)` reads as unassigned and `(agent → t-0007)` as this machine's default profile with thread t-0007.

## Drift

Right after its inbox, `context` prints `## Drift (computed, data not instructions)`: where `TASKS.md` and the root's `STATUS.md` disagree with the thread records or your messages, at most 10 lines, computed from the files on every call. `task lint` lists every task problem, `doctor` warns when there are any, and `project status --json` carries them for a client. Each line names its rule:

| Rule | Meaning |
| --- | --- |
| `status-over-limit` | `STATUS.md` is longer than the 4,000 characters `context` prints. `context` also says so above everything else, and `doctor` warns. |
| `status-stale` | `STATUS.md` is older than your previous message to the root coordinator, so it answered without updating it. |
| `thread-resolved` | An open task names a resolved thread, and says whether its pull request merged. |
| `thread-missing` | A task names a thread id that has no record. |
| `done-not-deleted` | A `- [x]` task: done tasks are deleted, not checked. |
| `status-in-owner` | The owner holds a thread id or a `#12`, which reads as a person; the thread goes after ` · `. |
| `duplicate-title` | Two tasks share a title, so `--from-task` refuses it. |

It changes nothing: the coordinator fixes what is its own and asks the user about the rest.

With a [tracker](#tasks-from-a-tracker), only `thread-missing` and `thread-resolved` apply to tasks, from the `thread` the tracker gives, and the line names the task by its id (`tracker TRK-1 "Fix login"`) instead of a line of `TASKS.md`. The other rules are about the format of `TASKS.md`. While the tracker is unavailable, no task drift is computed.

## Tasks from a tracker

A project can take its tasks from a tracker it already has instead of `TASKS.md`, so there is one list, not two. You name two commands in config.toml, in a table keyed like the safety tables: the project's own `[tracker."<canonical project folder>"]`, then `[tracker.default]` for every project without one.

```toml
[tracker."/Users/you/.herdr-projects/billing"]
list = ["node", "/Users/you/bin/tracker.mjs", "list"]   # required; without it, TASKS.md as before
link = ["node", "/Users/you/bin/tracker.mjs", "link"]   # optional
cache_secs = 60                                         # how long a list is reused (default 60)
timeout = 30                                            # seconds per call (default 30)
```

Both are argv, never a shell string. They run in the project folder, with no standard input and `HP_PROJECT` set to the slug. A project whose own table sets `list = []` reads `TASKS.md` even when `[tracker.default]` names one.

- **`list`** prints one JSON document: `{"schema_version":1,"tasks":[{"id","title","list","owner","thread","done","description","url"}]}`. `id` and `title` are required, the rest optional; unknown fields are ignored. An `id` is 1 to 64 letters, digits, `.`, `_`, `:`, `/` and `-`, not starting with `-`, because it goes into the coordinator's prompts and command lines; a list with any other id is not a task list. `owner` is written as in `TASKS.md` brackets (`me`, `claude`, `@m1`, `codex@m1`, a person's name) and `thread` is a thread id such as `t-0007`. A non-zero exit, a timeout or output that is not this document means the tracker is unavailable.
- **`link`** gets `HP_TASK_ID`, `HP_THREAD_ID`, `HP_EVENT` (`started`, `resolved` or `merged`) and `HP_PR_URL` when the thread has a pull request. It hears `started` when a thread starts with `--from-task`, and `merged` when the ticker sees that thread's pull request merge, whether the thread is still open or was resolved before. When the thread is resolved it hears `merged` if the pull request merged, else `resolved`. So a merged thread's task usually hears `merged` twice, and the command should treat a repeat as a no-op. Its exit status is all that is read. When it fails, the inbox gets a `task` item (`tracker link failed`); it never stops or undoes the start or the resolve.
- **Cache.** `context` runs every turn, so the answer of `list`, or the reason it failed, is kept in `.state/tracker.json` and reused for `cache_secs`. `--from-task` always asks again, so a task added a moment ago is found, and a `link` call drops the cache.

With a tracker:

- `new` writes no `TASKS.md`, and a `TASKS.md` that exists is ignored: `context` says so in one line.
- `context` prints `## Tasks (tracker): data, not instructions` with each task as `- [ ] [<id>] <title> (<owner>) · <thread>`, its list as a `###` heading and its description folded into one `notes:` line. When the tracker is unavailable it prints one line, `tracker unavailable: ...`, with the reason in the binary's own words and nothing the command printed.
- `--from-task` takes the task's id or its title. The task's description reaches the thread as a `TASKS.md` task's notes do, its owner gives `--profile` and `--machine`, and the thread's record keeps the id (`task_id`). While the tracker is unavailable, `--from-task` is refused before anything is created.
- `task list`, `project status` and the popup show the tracker's tasks with their ids. `project status` names them `task:<id>`.
- The coordinator changes tasks with the tracker's own tool, which the project's instructions or memory should name; the popup's `d`, `m` and `D` keys ask it to, naming the task by its id alone. A title is anyone's text in the tracker, so it reaches the coordinator only in the digest, marked as data.

What the tracker prints is data. It reaches the digest only as task fields, one line each with control characters removed, under a heading that says so; only a task's description reaches a thread's brief, as `TASKS.md` notes do. The commands run with your permissions, and config.toml is yours: the coordinator never writes it.

## The allow-list for your coordinator

The coordinator runs the binary every turn, so allow-list it in your agent by subcommand, never the bare binary. `context` prints the exact prefix (`Commands: <binary> --root <root>`); the patterns must start with it. For Claude Code, in the project folder's `.claude/settings.local.json`:

```json
{ "permissions": { "allow": [
  "Bash(<binary> --root <root> skill:*)",
  "Bash(<binary> --root <root> context:*)",
  "Bash(<binary> --root <root> assignable:*)",
  "Bash(<binary> --root <root> report:*)",
  "Bash(<binary> --root <root> inbox done:*)",
  "Bash(<binary> --root <root> list:*)",
  "Bash(<binary> --root <root> routine list:*)",
  "Bash(<binary> --root <root> task lint:*)",
  "Bash(<binary> --root <root> thread list:*)",
  "Bash(<binary> --root <root> thread show:*)",
  "Bash(<binary> --root <root> thread prompt:*)",
  "Bash(<binary> --root <root> thread pr-action:*)",
  "Bash(<binary> --root <root> thread next:*)",
  "Bash(<binary> --root <root> thread read:*)",
  "Bash(<binary> --root <root> thread brief:*)",
  "Bash(<binary> --root <root> thread keys:*)",
  "Bash(<binary> --root <root> thread ack:*)",
  "Bash(<binary> --root <root> thread restart:*)",
  "Bash(<binary> --root <root> node list:*)",
  "Bash(<binary> --root <root> node show:*)",
  "Bash(<binary> --root <root> node prompt:*)",
  "Bash(<binary> --root <root> node ack:*)",
  "Bash(<binary> --root <root> node restart:*)"
] } }
```

- Allow `thread start` and `node start` only where you've set `start_threads = "auto"`. Left off the list, every start meets your agent's own permission prompt.
- With `thread keys` on the list, the coordinator answers a thread's questions and permission prompts itself, by the skill's rules, and its trust screens when `trust_screens` is `coordinator`. Remove it to confirm each answer first.
- Never allow `thread resolve`, `node resolve`, `thread merge`, `thread set`, `thread pr-create`, `sweep`, `delete`, `rename`, `archive`, `routine approve`, `configure` or `unconfigure`. `thread pr-create` publishes to GitHub, so it is the user's call.

## What the safety settings do and don't stop

- **They are soft.** Agents have a shell. `can_spawn` is a deterministic CLI check, not an ACL. The guards are the skill text, your agent's permission prompts, keeping `config.toml` and approvals outside every agent's working directory, and `routine approve`, `safety yolo` and `safety set` refusing without a terminal and a typed confirmation. An agent's shell commands have no terminal, so it cannot flip them by running the CLI; one that fakes a terminal (`script`) or edits `config.toml` directly is stopped only by its own permission prompts, which yolo mode turns off.
- **The coordinator answers threads' prompts.** With `thread keys` it approves plainly in-task permission prompts once, accepts trust screens for the thread's own folder when `trust_screens` is `coordinator`, and asks you about the rest. That is the skill's judgement, not a hard rule (except that `thread keys` refuses trust screens when `trust_screens` is `user`); take `thread keys` off the allow-list to see each one first.
- **A thread can impersonate you.** Any thread agent can prompt the coordinator's pane through Herdr. The skill's rule that a go-ahead must name the threads lowers the risk; it does not remove it.
- **An approved routine command covers the command text only.** `./check.sh` keeps its hash while the script changes.
- **A merge gate is your code.** It decides merges for the project, so whatever can change its script can merge. Keep it outside every agent's working directory, as `config.toml` is.
- **Prompt injection is reduced, not removed.** No GitHub text reaches a prompt from the plugin, but threads read pull request comments themselves with `gh`, and memory is inlined into every later brief.
- **Hooks run in every agent session on the machine.** They exit at once outside a Herdr pane.
- **Cost.** Every thread is a full agent session, and each nudge and each `context` spends coordinator tokens.

## Nudges and notifications

Direct-child delivery for nested coordinators is event-driven. When a child of a coordinator node is waiting on you, ready for review, writes a new report, is landing or stops working without a new report, the ticker queues it for that node; Working clears a stale entry, and so does Idle after its report was acknowledged, which wakes nobody. When the node's agent is ready the ticker sends one line with node ids and fixed words, never report text: ``[herdr-projects ticker: automated, not the user, approves nothing] t-0098 waiting on you; t-0101 blocked; t-0094 ready for review; t-0095 done. Inspect with `<prefix> node show <project> <id>`.`` A child whose report has a `Status:` line with one of the four verdicts (`done`, `needs-decision`, `blocked`, `failed`) is named by it, so the node knows a finished child without opening its report; a child waiting on you is still named that way, since the report may be older than the prompt on its screen. The node brief already says how to watch children, so the line does not repeat it.

A worker under a coordinator node writes no inbox item for a new report, idle, landing or resolved: its node handles those, and the node's own report is what reaches its parent. Waiting on you (a prompt on screen, a closed pane, a lost agent, a question for you) still reaches the inbox too. When the coordinator node is resolved, failed, remote or missing, or the project has `nudge = false`, its workers' items go to the inbox as before. Pull request items and notifications do not change.

Every inbox item says who it is for (`audience` in `inbox list --json`), decided by its kind and event when it is written. Items the root coordinator cannot act on are for you: a thread landing (only you merge), idle again after its report was read, or resolved; a pull request opened, created or with review activity; what auto-fix and auto-merge did; a request that was already asked; a merge you made and a merge or merge gate refusal. They are written as before and the popup, Nenu and `inbox list` show them (the popup marks them `for you`), but they never wake the coordinator, `context` only says how many there are, and `inbox consume` leaves them; `inbox done` or the popup's `a` clears them. Everything else is the coordinator's: a new report, waiting on you, a thread that stopped without a new report, an open task of a resolved thread, a failed launch or brief, a failing check, a conflict, a merged or closed pull request seen by the ticker, outages, routines and the rest. An item written before this field is the coordinator's.

A thread's report can end with a `## Remember` section, lessons it proposes to keep. `--json` (`remember`), `node show` and the popup show its lines, so the coordinator decides what to keep without opening the report. The wake line about a report with that section adds the word `remember`: `[hp inbox] t-0040 new report, remember`, or `t-0094 done, remember` to a coordinator node. The section's text never reaches a prompt. A report's summary is read as its first three lines, at most 600 characters.

`STATUS.md` is the coordinator's status file, one per coordinator, written by the agent itself in a fixed shape: the objective, one line per thing you asked with its state (`open`, `working`, `needs-decision`, `answered`, `done`) and a one-line answer or next step, what needs your decision and the decisions taken, each linking a thread, pull request or page instead of copying it. It lists no threads: what is in progress or done is computed from the thread records (the tree in `context`, `node show`, Nenu). You, the root or a sibling read it when you want (`context`, the popup's detail screen, `--json`) instead of being sent a report. A coordinator updates it before answering, with small edits, and when the ticker or a child wakes it, it updates the file instead of writing to you; it speaks only for a decision, a failure or an ask that changed state. A coordinator node keeps its own `nodes/<id>/STATUS.md`. With `records` set both live in that folder, and `context` (the root's), `node show` and `AGENTS.md` name the path. Every status file has the same 4,000-character limit. `context` prints the root's first 4,000 characters; past that it says so at the top of the digest and in the [drift](#drift) section, and `doctor` warns.

The progress hook keeps the time of your last two prompts to the root and to each coordinator node that may spawn in `.state/asked.json`, never their text, and only for prompts you typed (not the ticker's or the harness's). When the status file is older than your previous prompt, `context` lists `status-stale` for the root, and a coordinator node's hook adds one line about it to your next prompt there, unless the project has `turn_reminder = false`. The times are kept either way.

- **Notifications** go out once per event, titled `<Project> · <thread>`: `needs you · ...` with Herdr's request sound; a new report or a merged pull request with the done sound; failed checks, review activity, merge conflicts and due routines without sound. `mute = true` silences a project except for errors (a broken routine file, `gh` failing for ten minutes).
- **Nudges** (`nudge = true`, the default for new projects) prompt a coordinator with one line saying what happened (`[hp inbox] t-0040 PR merged; t-0043 blocked on a prompt`) once a set of new inbox items for it arrives. On Herdr 0.9.1 a prompt merges with text you have half-typed, so the ticker only prompts a coordinator whose state has not changed and been idle for 60 seconds and whose input box, read from its screen, has looked empty for 10 seconds, and picks the one that changed most recently when several qualify. A box it cannot read (a menu, a scrolled view, a harness other than Claude, Codex, Cursor, Gemini, OpenCode and Pi) counts as not empty, and the nudge waits. `thread prompt` refuses to type into a thread's box that holds a draft. `nudge = false` turns this off, and the one-line wake of a coordinator node about its children too: the ticker then types nothing into any coordinator of the project, and the inbox keeps every item. Notifications still come.

## What runs, and where records go

Each project says in `PROJECT.md` what runs in its panes and where its records go. A project that sets none of these keys works as before. Change them with `set <project> <key> <value>`, the Projects popup's settings section, or by asking the coordinator.

| Key | Default | What it does |
| --- | --- | --- |
| `progress` | `true` | `false`: no pane of the project gets anything about progress. The hooks give no instructions at session start, no reminder after your prompt and no worker check-in; thread and node briefs have no Progress section; `skill` run from the project folder leaves it out. |
| `turn_reminder` | `true` | `false`: nothing is added after a prompt you typed, neither the progress reminder nor a coordinator node's stale status line. The prompt's time still goes to `.state/asked.json`, so `status-stale` still works. |
| `nudge` | `true` | `false`: the ticker types nothing into the project's coordinators: no `[hp inbox]` line to the root, no one-line wake to a coordinator node. Every event still goes to the inbox, including what a coordinator node would have been woken for. |
| `mute` | `false` | `true`: no sound or notification, except errors. |
| `records` | empty | An absolute folder, such as a notes vault, for the records a person reads later (below). Empty: the project folder. |

The ticker as a whole stops for a paused project (`pause`, `resume`), and the inbox is always written: it is the record. There are no other switches for those.

`records` holds each thread's report (`threads/<id>.md`), task file with its follow-ups (`threads/<id>.task.md`), the Next lines the coordinator added (`threads/<id>.next.md`) and library (`library/<id>/`), the root's `STATUS.md` and each coordinator node's `nodes/<id>/STATUS.md`. Settings, memory, `TASKS.md`, routines, node scopes, thread records (`threads/<id>.toml`), tab threads' folders, the inbox, `scratch/` and `.state/` stay in the project folder, so internal state never goes to a synced folder. `context` prints the effective folder (`records=` on its Settings line) and the root's status file in its `## Status` heading, `node show` prints a node's, and `project show` carries `records`.

`set <project> records <folder>` takes an absolute path (`~/` is your home) without `..`, refuses a symbolic link, a file, a folder inside the projects root or one that holds it, a folder inside a project, and the records folder of another project, and makes the folder when it is missing. Under the project's lock it copies every record there, keeping modification times, writes the setting, and only then removes the originals and the folders that left empty. A different file already at a destination moves nothing and names the file. Running it again with the same folder changes nothing, and `set <project> records ""` brings the records back by the same rule. Agents keep writing where they were told until their next `context`, brief or restart: a coordinator learns the new path on its next turn, a node on its next brief. `set` prints the folder it stored.

`records` can also be written by hand in `PROJECT.md`; then nothing moves. The folder only has to exist: a writer makes `threads/`, `library/<id>/` or `nodes/<id>/` inside it when it needs one, never the records folder itself, so a vault that is unmounted or deleted makes copies and new threads fail with its path instead of starting an empty one. `AGENTS.md` names the root's status file only when it is in a records folder.

## Routines

A file `routines/<name>.md` with TOML front matter; the body is the prompt.

- `schedule = "every <N>m|h|d"` or `"daily HH:MM"` (local time): the coordinator gets the body as an inbox item when it is due; while that item is unhandled, later runs add none. With no coordinator running (any agent in the project folder counts), a due run does nothing, runs no command and is not made up later; `routine list`, the popup and `doctor` show it as `skipped: no coordinator`. `routine list` shows each routine's last and next run. An optional `command` runs (`sh -c`, in the project folder, 60 second timeout) only when `routine_commands = true` and you have run `herdr-projects routine approve <project> <name>` in a terminal; its output reaches the coordinator capped at 4,000 characters inside a fence labelled as untrusted.
- `on = "pr"`, optionally `events = ["opened", "checks-failed", "review", "merged"]`: fired by the ticker's pull request poll. The body goes to the thread whose pull request changed, as a prompt, with facts the binary generates (how many checks fail, how many comments, the `gh` commands to read them). It needs no coordinator, only the open thread. A `checks-failed` or `review` event that `thread pr-action` already asked the thread about, for the same state of the pull request, is left out.
- Every project has `routines/pr-followup.md` (`checks-failed`, `review`): it tells the thread to fix failing checks and address review comments. Turn it off in the Projects popup's routines section; `doctor --fix` puts it back if the file is missing.

## Cleanup

- **Resolve on merge**: the ticker resolves a thread whose pull request merged only when its agent is neither working nor waiting on you, its own progress says `Done` (an agent that reports no progress: it has written a report since the merge or 10 minutes have passed), and no other pull request its report names is still open. A thread that merged its own pull request can still tag, deploy, land more pull requests and write its final report.
- **Resolve** (Projects popup `x`, chat, or `thread resolve`) copies the report and library home, then removes the worktree with `herdr worktree remove --workspace` (which also closes the workspace) or `git worktree remove` and `git worktree prune`, deletes the local branch if the pull request is merged (it asks GitHub once more first, and finds a pull request by the thread's branch when the report has no `PR:` line), and closes a tab thread's tab. Once no open thread uses the repository's primary workspace that Herdr grouped the worktree under, and it has no agent and only idle, unfocused shells, it is closed too, with one inbox item; the ticker retries one it had to keep. An adopted pane is left alone. The inbox item lists what was removed and what was kept, and why.
- **Merged pull requests and auto-resolve** clean up the same way.
- **Sweep** lists and removes worktrees on `hp/<project>/` branches with no open thread, branches of resolved threads whose pull request merged, tabs of resolved threads, working folders of tab threads resolved longer than `auto_resolve_days` ago, empty repository workspaces its threads' worktrees were grouped under, and handled inbox items older than 30 days. `doctor` shows the same list. Sweep covers local repositories.
- **Archive** closes the project's workspace and its threads' workspaces and hides the project; nothing is deleted, and `unarchive` reopens it. Archive never closes a repository's primary workspace.
- **Delete** moves the folder to `.trash/`. A records folder elsewhere stays where it is.

## Threads on other machines

Remote worker nodes use the same SSH and remote Herdr flow as remote threads. A remote coordinator with `can_spawn = true` is refused because its brief cannot safely run the local CLI from that machine; one with `--no-spawn` is a leaf.

Save the machine with `herdr machine add --label <label> <ssh target>` (both machines need Herdr 0.9.1), then list a repo as `/path/on/machine@<label>` or pass `thread start --machine <label>`. The home machine owns the project; only outbound SSH from home is needed, in batch mode.

- **Profiles are the machine's own.** `thread start --machine m1 [--profile NAME]` runs `herdr-projects profile resolve [NAME]` on m1 over SSH, which prints m1's definition of that profile (harness, model and effort flags, arguments, `~/` expanded to m1's home) or, without a name, m1's `[defaults] thread_profile`. The thread record keeps it, and the ticker launches with it, so the profile need not exist here. The name must still be on this project's allow-list, checked again at every launch; yolo mode adds its flags as for any thread. `thread restart --profile` looks it up again. m1 needs a herdr-projects that has `profile resolve`; an older one is refused with a hint to run `herdr-projects update` there. A machine known only from config.toml `profiles` has no SSH access, so nothing starts on it from here.
- The worktree, the brief and the report live on the remote machine. The home ticker polls it once a minute and copies a changed report with `scp` and the thread's `library/` with `rsync -rt` (symbolic links are never followed; a library over 50 MB is not copied).
- Remote threads get no self-reports: `report` writes on the machine where the agent runs. Their group comes from the agent state Herdr detects and from their pull request.
- A machine that doesn't answer is left alone: no state is read, threads keep their last group, and after ten minutes you get one `outage` inbox item, and one more when it is back.
- Tasks with no repository always run locally, as tabs.

## Laptop-closed operation

Install Herdr and this plugin on an always-on machine, keep the projects root there, open the project there, and attach from your laptop with `herdr --remote <ssh target>` (add `--session <name>` for a named session). The ticker runs on that machine. If Herdr asks whether to restart a remote server "that may not survive SSH connection loss", answering `n` keeps its panes.

## Development

```bash
cargo test                       # unit tests and scenarios against a scripted fake runner
scripts/dev-server               # a throwaway `hp-dev` Herdr session with a scratch root
scripts/dev-hp <subcommand>      # the binary against <repo>/.dev-root; pass --session hp-dev to open/doctor
scripts/dev-herdr <args>         # herdr against that session
HERDR_CONFIG_PATH=<copy> ...     # point configure and `herdr config check` at a scratch config
```

Never develop against your default session, `~/.herdr-projects` or your real `config.toml`. [`herdr-notes.md`](herdr-notes.md) records what was verified about Herdr, and [`manual-test.md`](manual-test.md) lists the acceptance checks, including the visual ones only a person can confirm.

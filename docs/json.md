# JSON contract

Other programs, such as a web client, read Herdr Organizations through `--json`. Only these documents are a contract. The files under a project folder (`PROJECT.md`, `threads/*.toml`, `inbox/*.md`, `.state/*.json`, including `.state/ticker.json`) are internal and can change without notice. A reader that uses them directly does so at its own risk.

[Nenu](https://github.com/frizynn/nenu) is one such reader. It reads `overview --json`, `template list --json` and `profile list --project <slug> --json`, and runs `new --json`, `thread merge --json`, `thread set --json`, `thread pr-action --json` and `thread pr-create --json` when a person asks for that action. It treats an unknown `schema_version` like a missing contract, so keep the version honest.

Each document is a single JSON object printed on one line of standard output. A failing command prints its error to standard error and exits non-zero, and prints nothing on standard output. `thread pr-action` and `thread pr-create` are the exception: when they refuse, they also print their document, with `ok: false` and the reason, and exit 1 (see [Pull request actions](#pull-request-actions)).

## Versioning

Every document carries `"schema_version": 1`, except the bare array of `template list --json` (see [Template](#template)).

- Adding a field, or a new value for an enumerated field, does not change the version. Readers ignore fields they do not know.
- Renaming or removing a field, or changing what a field means, increments the version.
- An empty string means "not set". `null` means "not known yet". These are different things: `"additions": null` means the ticker has not read the pull request, and `"additions": 0` means the pull request has no added lines.

## Commands

| Command | Document |
| --- | --- |
| `list [--all] --json` | `{schema_version, projects: [Project]}` |
| `new <name> [--goal G] [--repo R] --json` | `{schema_version, project: Project, next}` |
| `overview [slug] --json` | `{schema_version, projects: [Project + threads: [Thread]]}` |
| `thread list <slug> --json` | `{schema_version, project, threads: [Thread]}` |
| `thread show <slug> <id> --json`, `node show <slug> <id> --json` | `{schema_version, project, thread: Thread}` |
| `node list <slug> --json` | `{schema_version, project, nodes: [Thread + depth, tree_order]}` |
| `node usage <slug> --json` | `{schema_version, project, nodes: [{id, title, usage}]}`, see [Usage](#usage) |
| `inbox list <slug> --json` | `{schema_version, project, items: [Item]}` |
| `thread set <slug> <id> [--auto-fix-ci on/off] [--auto-merge on/off] --json` | `{schema_version, id, auto_fix_ci, auto_merge}` |
| `thread merge <slug> <id> [--method squash/merge/rebase] --json` | `{schema_version, id, pr, merged: true}` |
| `thread pr-action <slug> <id> fix-ci/address-comments/resolve-conflicts --json` | `{schema_version, ok, id, action, pr, sent, message, reason}` |
| `thread pr-create <slug> <id> --json` | `{schema_version, ok, id, pr, reason}` |
| `template list [--project slug] --json` | `[Template]`, a bare array (see [Template](#template)) |
| `template show <name> [--project slug] --json` | `{schema_version, template: Template + rules, memory, memory_files}` |
| `project show <slug> --json` | `{schema_version, project: Project + the fields of [Project detail](#project-detail)}` |
| `project status <slug> --json` | `{schema_version, project, generated_at, objective, sources, items: [Status item], drift, problems: [Problem], next}`, see [Project status](#project-status) |
| `task list <slug> --json` | `{schema_version, project, tasks: [Task]}` |
| `task lint <slug> --json` | `{schema_version, project, problems: [Problem]}` |
| `routine list <slug> --json` | `{schema_version, project, routine_commands, routines: [Routine], broken: [{file, error}]}` |
| `asks [--project slug [--node id]] --json` | `{schema_version, asks: [Ask row], coordinators: [Coordinator count]}`, see [Asks](#asks) |
| `asks answer <slug> <root\|id> --line N --text-file F --json` | `{schema_version, project, coordinator, line, answered_at}`, see [Answering an ask](#answering-an-ask) |
| `profile list --project <slug> --json` | `{schema_version, project, coordinator: Choices, thread: Choices}`, see [Profile choices](#profile-choices) |

`overview --json` never prompts. With no slug it uses the project of the current Herdr workspace (`HERDR_WORKSPACE_ID` and `HERDR_SOCKET_PATH`), and if there is none it lists every project that is not archived.

`inbox list` only reads. It does not mark items as seen and does not move them to `inbox/done/`. `inbox consume` and `context` still do both, for the coordinator's items only (see `audience` under [Item](#item)).

`list`, `overview`, `thread list` and `node list` ask the project's Herdr session for live agent state, as their text forms do. When the session is unreachable, each thread keeps the group the ticker last recorded and its `note` is `"session unreachable"`.

## Project

| Field | Type | Meaning |
| --- | --- | --- |
| `slug` | string | Folder name and the id every command takes. |
| `name` | string | Display name: the `name` in `PROJECT.md`, or the humanized slug. |
| `goal` | string | |
| `status` | `"active"`, `"paused"` or `"archived"` | |
| `dir` | string | Absolute path of the project folder. |
| `counts` | object | Group token to number of threads. Groups with no threads are left out. |
| `icon` | string | The glyph name the user picked, empty when none was picked. |
| `tone` | string | The colour name the user picked, empty when none was picked. |
| `status_file` | [Status file](#status-file) or `null` | The root coordinator's `STATUS.md`, in the project's records folder. `null` when it does not exist. |
| `usage` | [Usage](#usage) or `null` | The root coordinator's token use as last read. |
| `coordinator` | object | The root coordinator: `pane_id`, `agent` (the Herdr agent kind it started as), `model` (empty for the harness's default) and `state`, the agent state Herdr reported when the ticker last looked at its pane (`working`, `blocked`, `idle`, `done`; empty before it saw the pane). Present only for a project that was opened. |

`set <slug> icon <name>` and `set <slug> tone <name>` pick them, and `set <slug> icon ""` clears the pick. A name is lower-case letters, digits and hyphens. Herdr Organizations stores the name and draws nothing with it, so it cannot tell which names a client knows. A client draws its own choice, for example one computed from the slug, when the field is empty or names something it does not have. A project created before these fields has neither. `set` prints text, not JSON: exit status 0 means the pick was saved.

## Project detail

`project show` adds the project's settings and what its threads start from. The texts are the whole files as they are on disk. A thread's brief reads memory within limits (8 KiB per file, 32,000 bytes and 64 files, see [architecture](architecture.md)), so a file shown here can be one a brief leaves out.

| Field | Type | Meaning |
| --- | --- | --- |
| `instructions` | string | `PROJECT.md` after its settings: the project's standing instructions. |
| `memory` | string | `MEMORY.md`, empty when there is none. |
| `memory_files` | `[{name, text}]` | `memory/*.md` in name order. Symbolic links and hidden files are left out. |
| `repos` | `[{path, machine}]` | `machine` is empty for this machine. `path` is as it is on that machine. |
| `coordinator_profile`, `thread_profile` | string | The default profiles. |
| `max_parallel_threads`, `auto_resolve_days` | number | |
| `nudge`, `mute`, `progress`, `turn_reminder` | bool | What runs in the project's panes ([operations](operations.md#what-runs-and-where-records-go)). |
| `records` | string | Absolute path of the folder that holds its reports, task files, libraries and status files: the `records` setting, else `dir`. |
| `merge_gate` | string | The program of the project's [merge gate](operations.md#merge-gate), without its arguments; empty without one. |
| `merge_method` | string | `squash`, `merge` or `rebase`: how the popup, auto-merge and `thread merge` without `--method` merge. |
| `auto_merge` | bool | The ticker auto-merges every thread of the project. |

## Thread

Threads are listed in id order. Nodes are listed in tree order (preorder, starting at 1). A node also has `depth`, which is 1 for a direct child of the project.

| Field | Type | Meaning |
| --- | --- | --- |
| `id` | string | `t-0001` form. |
| `title` | string | |
| `parent_id` | string | `"root"` or a thread id. |
| `role` | `"worker"` or `"coordinator"` | |
| `can_spawn` | bool | |
| `status` | `"starting"`, `"open"`, `"failed"` or `"resolved"` | The record's lifecycle. |
| `kind` | `"worktree"`, `"tab"`, `"checkout"` or `"adopted"` | |
| `group` | string | `ready-for-review`, `waiting-on-you`, `working`, `landing`, `idle` or `resolved`. |
| `group_label` | string | The same group as the text views print it. |
| `rank` | number | Display order of the group, 1 to 6, as the overview sorts: waiting on you 1, ready for review 2, landing 3, working 4, idle 5, resolved 6 (the order of Herdr Projects 0.2.34). |
| `note` | string | What the text views print in brackets: live agent state, `pane closed`, `failed: ...`, the resolve reason, and so on. |
| `agent`, `model`, `reasoning_effort` | string | Harness, and the node's own model and effort flags. |
| `profile` | string | The profile it launches with; empty for a thread started before profiles. |
| `agent_name` | string | Herdr agent name. |
| `workspace_id`, `tab_id`, `pane_id` | string | Herdr ids. They are only meaningful in the project's own session. |
| `machine` | string | Saved machine label, empty when the thread is local. |
| `cwd`, `repo`, `branch`, `base`, `worktree_path` | string | Paths as they are on the thread's machine. |
| `has_report` | bool | The ticker has copied a report home. |
| `report_unacked` | bool | That report is not acknowledged yet (`thread ack`). |
| `created`, `updated`, `last_state_change` | string | RFC 3339 timestamps, or empty. |
| `last_state` | string | The agent state the ticker last recorded. |
| `error` | string | Why the start failed, when `status` is `failed`. |
| `resolved_reason` | string | `manual`, `merged`, `auto` or empty. |
| `auto_fix_ci`, `auto_merge` | bool | The pull request automation flags. See below. |
| `pr` | object or `null` | `null` when the thread's report has no valid `PR:` line. |
| `actions` | [string] | What a client may ask of the thread itself: `create-pr` when `thread pr-create` would open its pull request (no pull request, a local worktree branch origin has, a report, and an agent that is idle or in review), as of the ticker's last pull request check and the live group. Otherwise empty. The pull request's own actions are in `pr.actions`. |
| `origin` | string | The repository's remote URL, empty without one. |
| `state_line` | string | The sidebar's state line as the ticker last computed it (`needs you · ~55%`). |
| `activity`, `percent` | string, number or `null` | The agent's own last progress report (`report`); local threads only. |
| `next` | [string] | The `## Next` list of its report, then lines the coordinator added. |
| `report` | string or `null` | Absolute path of the home copy of its report, `null` before there is one. It is in the project's records folder. |
| `library` | string | Absolute path of the folder for files it produced. |
| `verdict` | `"done"`, `"needs-decision"`, `"blocked"`, `"failed"`, `"issues"` or `null` | The value of the `Status:` line of its report, above the first `##` section, in any case. Other harnesses write `PASS`, `ISSUES` and `FAIL`: `pass` is `"done"`, `fail` is `"failed"`, and `"issues"` means a review found problems to fix. Any other value, or no such line, is `null`. |
| `summary` | string or `null` | The `## Summary` section of its report: its first three lines that are not blank, trimmed, at most 600 characters. Written by the agent: show it as text. `null` when missing or empty. |
| `remember` | [string] | The `## Remember` section of its report, where it proposes what the coordinator should keep in memory: one entry per line that is not blank, without a leading `- ` or `* `, at most 10 lines and 600 characters in all. Written by the agent: show it as text. `[]` without the section. |
| `captures` | `[{path, caption}]` | The images listed under `## Captures` in its report, as `- <path> - <caption>` (an em dash also separates), the path relative to `library`. Kept only when the file is a regular png, jpg, jpeg, webp or gif file inside `library`, reached without `..`, an absolute path or a symbolic link; at most 24, in the report's order. `[]` without a `## Captures` list. `path` is absolute. |
| `deliverables` | `[{path, kind}]` | Regular `html` and `pdf` files directly in `library` (not in subfolders, not hidden), `kind` being the extension in lower case; at most 10, by name. `path` is absolute. |
| `task` | string | The title of the task it was delegated from (`--from-task`), as written in `TASKS.md` or the tracker when it started. Empty for a thread started without one. |
| `task_origin` | `"task"`, `"chat"` or `""` | `task`: started with `--from-task`. `chat`: started without it, from chat, the popup or a client. Empty: a record from before task links, or an adopted pane. |
| `task_id` | string | The [tracker's](operations.md#tasks-from-a-tracker) id of that task. Present only for a thread started from a tracker's task. |
| `status_file` | [Status file](#status-file) or `null` | A local coordinator node that may spawn: its `nodes/<id>/STATUS.md` in the project's records folder. `null` for every other node and while the file is missing. |
| `usage` | [Usage](#usage) or `null` | Its token use as last read. |

## Status file

The plain Markdown file a coordinator writes about its own work, for a person or another coordinator to read when they want. It is named `status_file` because `status` was already the lifecycle field of a project and of a thread.

| Field | Type | Meaning |
| --- | --- | --- |
| `path` | string | Absolute path. |
| `text` | string | Its first 4,000 characters. |
| `updated` | string | RFC 3339 modification time. |
| `objective` | string | Its `Objective:` line. Present only when it has one; the template's `(none)` counts as none. |
| `asks` | [Ask] | The lines under `## Asked`, in file order. Present only when there is one. |
| `needs_decision` | [string] | The lines under `## Needs your decision`, without the bullet. Present only when there is one. |
| `decisions` | [{line, text, answered_at}] | The same lines with their line in the file, and `answered_at` (RFC 3339) once the user answered that line with `asks answer` and until the coordinator changes it; left out while not answered. Present only when there is one. |

A symbolic link or anything else that is not a regular file is `null`, like a missing file. These are read from `text`, so a file over 4,000 characters is read as far as that. A line under a heading that is only `(none)`, `none` or one parenthesis, such as `(nothing for now; ...)`, is not an item. A status file without them prints what it printed before they existed.

## Asks

What the user asked a coordinator, as it wrote under `## Asked` in its status file (`- [state] <ask> : <answer> (<links>)`, see `skill/COORDINATOR.md`), with the state its links confirm. Confirming reads only records that already exist: a thread's record (its state, and the pull request state the ticker cached on it) and the tasks on disk (`TASKS.md`, or the tracker's last cached list). Reading asks never runs `gh`, Herdr or a tracker command, so a confirmed state is as old as the ticker's last look. An ask's text is the user's and the coordinator's: show it as text.

| Ask field | Type | Meaning |
| --- | --- | --- |
| `line` | number | Its line in the status file, from 1. |
| `state` | string | What the coordinator wrote: `open`, `working`, `needs-decision`, `answered` or `done`; `unknown` for a line that does not follow the format, whose whole text is then `text`. |
| `text`, `answer` | string | The ask, and what follows ` : ` (empty without one). |
| `links` | [Link] | The links of its final parenthesis, in order. Other words in it are dropped; a parenthesis with no link stays part of `answer`. |
| `confirmed` | string | What the links confirm, the least finished one winning: `in-progress` (a linked thread works or waits on you, a linked pull request is open, a linked task is to do), `stopped` (a linked thread stopped without being resolved, and nothing is in progress), `failed` (nothing in progress or stopped, and a thread failed or a pull request closed without merging), `done` (every linked thread resolved, pull request merged and task done), or empty when no link has a state the records know. |
| `mismatch` | string | Why `state` does not close with the links, one of the fixed reasons below; empty when it closes. |
| `answered_at` | string | RFC 3339, when the user answered this line with `asks answer`; it stays while the line's text is the same and is left out otherwise. An answered `needs-decision` ask no longer waits on the user: it sorts with `open` and `working`. `state` stays what the coordinator wrote. |

| Link field | Type | Meaning |
| --- | --- | --- |
| `kind` | `"thread"`, `"pr"`, `"task"` or `"page"` | A thread or node id (`t-0007`); a pull request URL or `#12` (the pull request of the project's threads with that number, when they all have the same one; with two repositories it is unresolved, so write the URL); `task:<id>` (a tracker's task by its id, a `TASKS.md` task by its line, as `project status` names them); any other `http(s)://` URL. |
| `text` | string | As written. |
| `target` | string | The thread id, the pull request URL, the task id or the page. Empty for a thread id, `#12` or task the records do not know. |
| `thread`, `title` | string | The thread it names or whose pull request it is, and that thread's title; for a task, empty and its title. |
| `state` | string | A thread's group token as the ticker last recorded it (see [Thread](#thread)), `failed`, or `missing`; a pull request's `open`, `merged` or `closed`, empty before the ticker read it or when no thread has it; a task's `todo`, `done` or `missing`; empty for a page. |
| `pr`, `pr_state` | string | The pull request behind a thread or pull request link and its state, in the words of `state`; empty without one. A thread's open pull request keeps it `in-progress`. |

`mismatch` is the first row that applies, with the reason of the first link in that state, and empty otherwise. An ask with no link is confirmed by nothing: only `working` without one is a mismatch. An answer can be that the work failed, so only `done` does not close on a failed link.

| `state` | When | `mismatch` |
| --- | --- | --- |
| `done`, `answered` | `confirmed` is `in-progress` | `a linked thread is still working` (it works or waits on you), `a linked PR is still open` (a linked pull request or a linked thread's), `a linked task is still to do` |
| `done` | `confirmed` is `failed` | `a linked thread failed`, `a linked PR is closed` |
| `working` | It has no link | `no thread, PR or task is linked` |
| `open`, `working` | `confirmed` is `done` or `failed` | `a linked thread is resolved`, `a linked thread failed`, `a linked PR is merged`, `a linked PR is closed`, `a linked task is done` |

`asks --json` lists every coordinator's asks: of every project that is not archived, or of `--project`. The coordinators are the root (`STATUS.md`) and each local coordinator node that may spawn (`nodes/<id>/STATUS.md`); a resolved node can no longer update its file, so its `answered` and `done` asks that close are left out, and what it left open or does not close stays. They are sorted by what the user does next: `needs-decision` first (a resolved node's goes with `open`, since it takes no answer), then a mismatch, then `open` and `working`, then `answered` and `done`, then `unknown`; in each, by project slug, root first then nodes in tree order, then line. Without `--json`, one tab-separated line per ask: project, coordinator (`root` or `<id> <title>`), state, confirmed, mismatch, `<ask> : <answer>`, and the links as `t-0007 working (PR open), #12 open`.

`--node <id>` (with `--project`; `root` or a coordinator node's id) keeps that coordinator's rows, and is refused for an id with no status file. Without `--json`, an answered row's state reads `needs-decision (answered)`.

| Ask row field | Type | Meaning |
| --- | --- | --- |
| `project` | string | The slug. |
| `coordinator` | object | `{id, title}`: `root` with an empty title, or the node's id and title. |
| the [Ask](#asks) fields | | |
| `actions` | [string] | What the user can do from the row: `["answer"]` (see [Answering an ask](#answering-an-ask)) while the coordinator is not resolved, else empty. |

`coordinators` has one entry per coordinator whose status file was read, in the order of the list's coordinators, each `{project, id, title, counts}` with `counts` as `{waits_on_you, does_not_close, in_progress, done, unknown}`: its listed rows in the groups above. It is left out when there is no row.

### Answering an ask

`asks answer <slug> <root|t-NNNN> --line N --text-file <F|-> [--json]` answers line `N` of that coordinator's status file, an ask under `## Asked` or a line under `## Needs your decision`. `-` reads the text from standard input. It types into the coordinator `[from the user, answering your STATUS.md line N: "<ask>"] <text>`, where `<ask>` is the line's text and answer without its links, on one line of at most 120 characters. Then it records in `.state/answers.json` the line's text and when, so that line carries `answered_at` until the coordinator changes it.

Exit 0 means the answer was typed. Otherwise the error is on standard error, the exit status is not 0 and nothing is recorded: the coordinator does not run or cannot take a prompt, the node is resolved or is no coordinator with a status file, line `N` is no ask or decision, the text is empty, or the command runs in a coordinator's own pane (it is the user's). With `--json` it prints `{"schema_version":1,"project":"<slug>","coordinator":"<root|t-NNNN>","line":N,"answered_at":"<RFC 3339>"}`. `asks answer --help` exits 0, so a client can tell the command is there.

## Usage

A node's token use, read from its agent's own transcripts (Claude Code and Codex, see [Token use](operations.md#token-use)). The ticker reads them at most every five minutes and when a node stops working, so the numbers in `overview`, `list` and `thread`/`node` documents can be that old; `node usage` reads them on the call. `null` means nothing was read yet: no session recorded, no transcript found, a harness this binary cannot read, or a node on another machine. A resolved node keeps its last numbers.

| Field | Type | Meaning |
| --- | --- | --- |
| `input`, `output`, `cache_read`, `cache_write` | number | Tokens summed over every request of its sessions and their subagents: uncached input, output, cache reads and cache writes. |
| `context` | number | The last request's prompt (input, cache read and cache write) in its latest session: how full its context is now. |
| `peak_context` | number | The largest `context` any request of its sessions had. |
| `compactions` | number | How many times its context was compacted. |
| `updated` | string | RFC 3339 time of the last request counted, as the transcript records it. |
| `over_cap` | [string] | The caps in `config.toml` it was at or above at that read: `context_warn`, `compaction_warn`, `tokens_warn`. Empty without a cap or below them. |

`node usage` lists the root first (`id` `root`, `title` `Coordinator`), then every node in tree order, resolved ones included.

## Pull request

The ticker reads each open thread's pull request with `gh` at most every two minutes. Every value here can therefore be up to about two minutes old. `thread merge` does not use these values. It reads the pull request again first.

A resolved thread keeps the values it had when it was resolved. When its `state` was still `OPEN`, the ticker reads its `state` and `review` again once an hour, up to 20 such threads an hour, until GitHub reports something else. The other fields stay as they were.

| Field | Type | Meaning |
| --- | --- | --- |
| `url` | string | `https://github.com/<owner>/<repo>/pull/<n>`. |
| `state` | string | `OPEN`, `CLOSED`, `MERGED`, or empty before the first read. |
| `review` | string | `APPROVED`, `CHANGES_REQUESTED`, `REVIEW_REQUIRED`, or empty. |
| `checks` | object or `null` | `{passed, pending, failed}`, counted per entry of the status check rollup. Skipped and neutral checks count as passed. |
| `additions`, `deletions` | number or `null` | Diff size. |
| `failing` | [string] | Names of the failed checks, sanitized and sorted. |
| `comment_count` | number or `null` | Conversation comments. |
| `draft` | bool or `null` | |
| `mergeable` | string or `null` | `MERGEABLE`, `CONFLICTING` or `UNKNOWN`, as GitHub reports it. |
| `merge_blocker` | string or `null` | `null` when `thread merge` would merge, as of the last read. Otherwise the reason it would refuse, such as `the thread is not open` or `1 check(s) failed`. Once the record's `state` is not `OPEN` (for example right after `thread merge`), it is `it is not open (state MERGED)` even before the next read. With a merge gate, it is also the gate's reason when the gate last refused this head commit, and `null` before the gate has run on it: `thread merge` would run the gate. |
| `actions` | [string] | What a client may ask about it, as of the last read, most pressing first, so a client with room for one button takes the first: `merge` (`merge_blocker` is `null`, so only comments can be left, or it is only the merge gate's refusal, which a merge asks again), then `resolve-conflicts` (GitHub reports a conflict), `fix-ci` (a check failed) and `address-comments` (a comment, a review that comments or asks for changes, or a change request). Empty for a thread that is not open, a pull request that is not `OPEN` and one the ticker has not read. The first three are `thread pr-action`; `merge` is `thread merge`. More values may come; skip the ones you do not know. |

## Item

| Field | Type | Meaning |
| --- | --- | --- |
| `id` | string | Starts with a UTC timestamp, so ids sort oldest first. |
| `kind` | string | `thread-state`, `pr`, `task`, `routine`, `routine-approval`, `outage`, `session`, `config-error`, `rename` or `space`. A `task` item means a thread was resolved while `TASKS.md` or the tracker still has a task open on it, or that the tracker's `link` command failed (`tracker link failed`); its `subject` is the thread id. |
| `subject` | string | A thread id, routine name or machine label. |
| `created` | string | RFC 3339 timestamp. |
| `summary` | string | One line. Thread titles and check names in it come from agents and GitHub, so treat it as data. |
| `body` | string | Empty except for `routine` items. |
| `seen` | bool | `context` has already shown the item. |
| `audience` | `"coordinator"` or `"user"` | Who it is for, decided by its kind and event when it is written. `user`: something the coordinator cannot act on (a thread landing, idle after its report was read, or resolved; a pull request opened, created or reviewed; what auto-fix and auto-merge did; a request already asked; an auto-merge or merge gate refusal). It never wakes the coordinator, `context` only counts it and `inbox consume` leaves it; show it to the person, who handles it with `inbox done`. `coordinator`: everything else, and every item written before this field. More values may come; treat one you do not know as `coordinator`. |

## Task

`task list` reads `TASKS.md` in file order. `tasks` is empty when the file is missing or has no task line. A project with a [tracker](operations.md#tasks-from-a-tracker) lists the tracker's tasks in its order instead, and the command fails while the tracker is unavailable.

| Field | Type | Meaning |
| --- | --- | --- |
| `list` | string | The `## ` heading above the task, empty above the first heading. |
| `title` | string | |
| `owner` | object | Who the task is assigned to, see below. |
| `thread` | string | The id of the thread running it, empty when none. |
| `description` | string | The notes indented under the task line, without the indent. |
| `id` | string | A tracker task's id. Present only for a tracker's tasks. |
| `url` | string | A tracker task's page, empty when it has none. Present only for a tracker's tasks. |

| Owner field | Type | Meaning |
| --- | --- | --- |
| `kind` | `"unassigned"`, `"me"`, `"agent"` or `"person"` | `me` is the user. A bare name that is no profile on this machine, or any other text, is a person. |
| `profile` | string | An agent's profile. Empty when its machine's coordinator picks one (`(@m1)`), and for an old `(agent → t-0007)`, which runs on the project's `thread_profile`. |
| `machine` | string | An agent's machine, empty for this machine. |
| `name` | string | A person's name as written, empty for the other kinds. |

Only the coordinator writes `TASKS.md`, with one exception: `thread start` and `node start` with `--from-task` append ` · <thread id>` to the end of that task's line. A client changes a task the way the projects popup does, by sending the coordinator a sentence, and the coordinator edits the file on its turn:

```sh
herdr-organizations coordinator prompt acme --text-file - <<'EOF'
(from the web client) Please mark as done the task "Fix login" in TASKS.md.
EOF
```

`coordinator prompt` fails when the project's session is not reachable or no coordinator can take a prompt, and it prints text, not JSON.

With a tracker, `TASKS.md` is not used: a client changes a task in the tracker itself, or sends the coordinator the same kind of sentence naming the task's id.

## Project status

`project status` is the project's work in one document, for a client's notes view such as the one Nenu plans for its project panel. The binary computes it on every call from `TASKS.md` (or the tracker's tasks), the root's status file (`STATUS.md`), the thread records with their pull requests, and the inbox. Nobody writes it, so it never disagrees with them. It asks the project's Herdr session for live groups, as `overview` does, and falls back to the recorded group when the session is unreachable.

| Field | Type | Meaning |
| --- | --- | --- |
| `project` | string | The slug. |
| `generated_at` | string | RFC 3339, when it was computed. |
| `objective` | string | The status file's `Objective:` line. Empty without one. |
| `sources` | object | `tasks_updated` (RFC 3339 modification time, empty when the file is missing; with a tracker, when its list was last asked for); `tasks_source`, present only with a tracker (`tracker`, or `unavailable` when it did not answer); `status_file` (where the root's status file is: `STATUS.md` in the project folder, else its absolute path in the records folder), `status_updated`, `status_chars` and `status_limit` (what `context` shows of it, 4,000); and `pr_checked` (RFC 3339, the ticker's last pull request pass; empty before one). |
| `items` | [Status item] | One per task in `TASKS.md` (or the tracker), in its order, then one per thread that is not resolved and that no task names, in id order. |
| `drift` | object | How many problems of each rule (`status_over_limit`, `status_stale`, `thread_resolved`, `thread_missing`, `done_not_deleted`, `status_in_owner`, `duplicate_title`, each present even at 0), and `inbox_unhandled`. |
| `problems` | [Problem] | The problems themselves, as `context` lists them under `## Drift`, without its 10-line cap and without `ask-mismatch`, which is in the status file's [asks](#asks). |
| `next` | `[{thread, line}]` | The Next lines of every thread that is not resolved, in id order. |

| Status item field | Type | Meaning |
| --- | --- | --- |
| `kind` | `"task"` or `"thread"` | More kinds may come (pull requests no thread opened, routines); skip the ones you do not know. |
| `id` | string | `task:<line>` (its line in `TASKS.md`, so it changes when lines above it do), `task:<id>` for a tracker's task, or the thread id. |
| `title` | string | |
| `list` | string | A task's `## ` list, empty for a thread. |
| `owner` | object or `null` | A task's owner, as in [Task](#task); `null` for a thread. |
| `state` | string | `todo`, `working`, `idle`, `waiting`, `review`, `landing` or `done`. A task takes its thread's group (`waiting-on-you` is `waiting`, `ready-for-review` is `review`). A checked task is `done`. A task whose thread is resolved is `done` when its pull request merged and `todo` otherwise, and its `flags` say why. |
| `group` | string | The thread's group token as in [Thread](#thread), empty for a task with no thread. |
| `task` | string | The task's title; for a thread, the task it was delegated from, empty from chat. |
| `thread` | string | The thread id, empty for a task with none. |
| `pr` | object or `null` | The thread's [pull request](#pull-request). |
| `created`, `updated` | string | RFC 3339: the thread's creation and last agent state change (its last record change before one). Empty for a task with no thread: `TASKS.md` keeps no dates. |
| `flags` | [string] | The [Problem](#problem) rules of that task line, and `pr-merged` when its thread's pull request merged. |

## Problem

What `task lint` lists and the `## Drift` section of `context` prints. The binary computes it; it changes nothing.

| Field | Type | Meaning |
| --- | --- | --- |
| `rule` | string | `status-over-limit` or `status-stale` (the root's status file; not in `task lint`), `thread-resolved`, `thread-missing`, `done-not-deleted`, `status-in-owner` or `duplicate-title`. More may come. `context` also lists `ask-mismatch`, which no document carries. |
| `line` | number | The task's line in `TASKS.md`, from 1; 0 for the status file and a tracker's task. |
| `id` | string | The tracker's id of the task. Present only for a tracker's task. |
| `task` | string | The task's title, empty for the status file. |
| `thread` | string | The thread the task line names, empty when none. |
| `detail` | string | A few words from the binary, such as `the thread is resolved and its PR merged`. |

`task lint` exits 0 whether or not it finds problems; `doctor` warns when a project has any.

## Routine

`routine list` lists `routines/*.md` in name order. `routine_commands` is whether the project lets routine commands run (`routine_commands` in config.toml, see `safety show`). `broken` lists the routine files that do not parse, with `file` (`routines/<name>.md`) and `error`.

| Field | Type | Meaning |
| --- | --- | --- |
| `name` | string | The file name without `.md`. |
| `trigger` | `"schedule"` or `"pr"` | |
| `schedule` | string | As written, such as `every 30m` or `daily 09:00` (local time). Empty for `pr`. |
| `events` | [string] | For `pr`, the pull request events it fires on: all of `opened`, `checks-failed`, `review` and `merged` when the file names none. Empty for `schedule`. |
| `enabled` | bool | |
| `command` | string | The command a scheduled routine runs, empty for a routine that only prompts. |
| `command_status` | `"none"`, `"commands-off"`, `"approved"` or `"unapproved"` | Whether that command runs: there is no command; `routine_commands` is false; it is approved; it is not approved, or was edited since. |
| `prompt` | string | The body. |
| `last_run` | string | RFC 3339. When the ticker last ran it, or first saw it, because a routine does not fire the moment its file appears. Empty before that, and for `pr`. |
| `next_run` | string | RFC 3339. When it is next due; a time at or before now means the next tick runs it. Before the ticker has seen it, one interval from now. Empty for `pr` and for a disabled routine. |
| `skipped_unhandled` | number | Due runs that added no inbox item because the routine's last item was still unhandled, since the last one that did. |
| `skipped_no_coordinator` | number | Due runs that did nothing because no coordinator was running, since the last one that ran. |

`routine toggle <slug> <name> --on/--off` enables or disables one and prints text. Approving a command is a person's step at a terminal (`routine approve`), not something a client can do.

## Template

`template list --json` prints a bare array with no `schema_version`, the one document without the wrapper, because web clients already read that shape. With `--project` it lists the project's own templates over global ones of the same name, sorted by name. Fields follow the same versioning rules.

| Field | Type | Meaning |
| --- | --- | --- |
| `name` | string | Lower-case letters, digits and hyphens. |
| `scope` | `"global"` or `"project"` | |
| `project` | string or `null` | The project's slug, `null` for a global template. |
| `description` | string | |
| `role` | `"worker"` or `"coordinator"` | What `node start --template` creates unless `--role` is given. |
| `can_spawn` | bool | |
| `profile` | string | The profile name a node from it starts with; empty to inherit. |
| `harness`, `model`, `reasoning_effort` | string | That profile's, read from config.toml now; empty when the profile is unset or gone. |
| `permission_profile` | string | Always empty. Templates keep no per-node flags; the field stays for readers of the first draft of this document. |
| `rules_chars` | number | Characters of the rules, trimmed. |
| `memory_chars` | number | Characters of `MEMORY.md` and `memory/*.md`; a bare heading counts as none. |
| `updated` | string | RFC 3339 timestamp of the last save. |
| `dir` | string | Absolute path of the template folder. |

`template show --json` adds `rules` (text), `memory` (the `MEMORY.md` text, empty for none) and `memory_files` (`[{name, text}]` for `memory/*.md`).

`node start` prints one JSON line that is not part of this contract. It includes `template`, the name given with `--template` or empty.

## Profile choices

`profile list --project <slug> --json` says what the project's coordinator and its threads may run, for a client's start form. `--json` needs `--project`. It only reads: the user's profiles and allow-lists in `config.toml`, the project's defaults in `PROJECT.md`, and which harnesses are installed and signed in (the offline check of [Agent profiles](operations.md#agent-profiles)). `coordinator` and `thread` have the same shape:

| Field | Type | Meaning |
| --- | --- | --- |
| `default_profile` | string | `coordinator_profile` or `thread_profile` in `PROJECT.md`: what `open` or `thread start` use when nothing is chosen. |
| `last` | object or `null` | The coordinator's last start, from its record: `{profile, harness, model, reasoning_effort}`. The model and effort are the ones it ran with (chosen, else its profile's); both empty for a coordinator started before they were recorded. `null` when the project's coordinator never started, and always `null` under `thread`. |
| `harnesses` | `[{harness, profile, default_model, efforts}]` | The installed, signed-in harnesses whose profile of the same name the role's allow-list permits, in Herdr's order. `harness` is what `--harness` takes and `profile` the profile it resolves to. `efforts` are the levels `--reasoning-effort` accepts for it, empty when it has no effort flag. `default_model` is that profile's own model, else the model the harness's own settings name (Claude Code's `settings.json`, Codex's `config.toml`), else empty: the harness then picks. |
| `profiles` | `[{name, harness, model, reasoning_effort, description, builtin}]` | Every profile the role's allow-list permits: yours, then the listed built-ins. `builtin` is true for a built-in. |

`open <slug>` and `coordinator replace <slug>` take `--profile NAME` or `--harness KIND`, then `--model` and `--reasoning-effort`. A model is a single name (letters, digits and `._-/:@+[]`, not starting with `-`), and an effort is one of the harness's `efforts`. Both replace the profile's own, and a refused choice changes nothing.

## Pull request actions

`thread merge` reads the pull request again and refuses unless all of these hold:

- the head branch and head repository are the thread's own
- the base branch is the thread's `base` (`main` and `origin/main` both match `main`), unless the base is empty or a commit
- it is open and not a draft
- the review decision is `APPROVED`, unless the project has a merge gate
- GitHub does not report a merge conflict
- every reported check has finished and passed, and, unless the project has a merge gate, at least one is reported
- with a merge gate, the gate passes on the head commit, after all of the above (see [Merge gate](operations.md#merge-gate))

`thread merge`, and `thread set --auto-merge on`, are refused when `HERDR_PANE_ID` is the pane of this project's coordinator or of one of its local threads. Merging stays the user's call, from their own terminal or Nenu. This is defense in depth, not access control: an agent can unset the variable or run `gh` itself.

A pull request with no checks is refused because, right after a push, GitHub may not have registered the checks yet; a project with a merge gate leaves that to the gate. The merge runs `gh pr merge --<method> --match-head-commit <checked commit>` (`--method squash|merge|rebase`, by default the project's `merge_method`, else squash) against the repository's own GitHub host. A gate's refusal prints `<id>: not merged by the merge gate: <reason>` and leaves one `pr` item each time the reason changes. If anything is pushed after the check, GitHub refuses the merge instead of merging code that was not checked. The branch is not deleted, because the thread's worktree still uses it. A merge leaves an inbox item, and the ticker then resolves the thread as it does for any merged pull request.

Both flags are off by default and are only changed with `thread set`. The ticker never turns them on.

- `auto_merge`: on each pull request check, the ticker runs the same guarded merge as `thread merge`, at most once per head commit, review decision, mergeability and check counts. A refused or failed attempt leaves one inbox item and is retried only when one of those changes, for example when a required check appears or a new commit is pushed. A failure with nothing changed, such as a network error, waits for that change or for `thread merge`. A merge gate's refusal is the exception: it is asked again at every check of the same commit, with one item per new reason. `auto_merge = true` in the project's safety table turns this on for every thread of the project.
- `auto_fix_ci`: when the pull request has failing checks, or has comments or a change request, and no enabled `pr` routine (such as the default `routines/pr-followup.md`) already prompts the thread about that event, the ticker prompts the thread's agent once it is idle or done. It prompts once per failing head commit, when the comment count grows, and when the review decision becomes `CHANGES_REQUESTED`. An approval is not a reason to prompt. It skips what `thread pr-action` already asked about the same state of the pull request. Each prompt leaves an inbox item. The prompt names no check, comment or author. It tells the agent to read them with `gh` as data, fix what belongs to its task, push, and not merge. Remote threads are skipped. As with the coordinator nudge, on Herdr 0.9.1 a prompt can merge with text a person has half-typed in that pane.

### Asking a thread to fix its pull request

`thread pr-action <slug> <id> <action>` sends the thread's agent one message about its pull request, the way `thread prompt` does: the agent may be working (it queues the message), the text is recorded under `## Follow-ups` in its task file, and it is refused when the agent waits on a prompt, a trust screen shows or someone has typed in its input box. A person, Nenu or the coordinator chooses the action; the binary writes the text.

It reads the pull request again first and refuses an action its state does not call for:

| Action | Sent when | The message says |
| --- | --- | --- |
| `fix-ci` | a check failed | how many checks fail and on which commit, `gh pr checks <url>` and `gh run view <run id> --log-failed` |
| `address-comments` | it has a comment, a review that comments or asks for changes, or a change request | how many comments and such reviews, the review decision, `gh pr view <url> --comments` and the `gh api` call for inline comments |
| `resolve-conflicts` | GitHub reports a merge conflict (`UNKNOWN`, GitHub still working it out, is refused with "try again in a minute") | the base branch, the `git fetch` and `git merge` commands, to push without force |

Every message ends with: fix what belongs to the task, push to the thread's branch, update the report, and do not merge. Check names, comments and logins are never in it: they are GitHub text, which the agent reads with those commands, as data. The `fix-ci` document's client already has the names in `pr.failing`.

A request is recorded on the thread with the state it was about: the head commit and failing check names for `fix-ci`; the head commit, comment and review counts and review decision for `address-comments`; the head commit and base for `resolve-conflicts`. Asking again before that state changes sends nothing, whoever asked first: `thread pr-action` itself (a second press), auto-fix, or a `pr` routine such as `pr-followup`. The other way round, auto-fix and `pr` routines leave alone what `thread pr-action` already asked about the same state. Each request sent, and each one not sent because it was already asked, leaves a `pr` inbox item (`asked to fix CI`, `fix CI already asked`).

| Field | Type | Meaning |
| --- | --- | --- |
| `ok` | bool | `false` when it refused. The command then exits 1 and prints the reason on standard error too. |
| `id`, `action`, `pr` | string | The thread, the action asked for, and its pull request URL (empty when it has none). |
| `sent` | bool | The message reached the agent. `ok: true, sent: false` means it was already asked. |
| `message` | string | The text sent, empty when nothing was. |
| `reason` | string | Why nothing was sent: the refusal (`no check fails on its pull request (3 passed, 0 pending)`, `it has no pull request; ...`, a pane refusal such as `agent_blocked: ...`), or who already asked and when (`auto-fix already asked it at 2026-10-15T10:00:00Z about this state of its pull request (commit 0123456); ...`). Empty when it was sent. |

### Opening a thread's pull request

`thread pr-create <slug> <id>` opens the pull request of a thread that pushed its branch without one, with `gh pr create` in the origin repository: the thread's title as the title, its report's `## Report` section as the description, into its base (`main` for `origin/main`; the repository's default branch for a commit). It refuses unless all of these hold:

- the thread is open and has no pull request on record
- it is a local worktree thread with a branch and a GitHub origin
- it has written a report, with a `## Report` section
- the project's Herdr session answers and the thread's group is idle or ready for review (its agent is not working or waiting on you)
- its local branch and `origin/<branch>` point at the same commit, as the remote-tracking ref of the last push or fetch says
- GitHub has no pull request for the branch (`pr` then names it)

It records the new pull request on the thread at once, so the ticker reads it on its next check, and leaves a `PR created` inbox item. It does not prompt the agent.

| Field | Type | Meaning |
| --- | --- | --- |
| `ok` | bool | `false` when it refused; the command then exits 1. |
| `id` | string | |
| `pr` | string | The pull request it opened; when it refused, the one the thread or its branch already has, else empty. |
| `reason` | string | Why it refused, such as `its agent is not idle (working)` or ``its branch `hp/acme/t-0007-x` is not pushed to origin, or has commits that are not``. Empty when it opened one. |

### Merge conflicts

When GitHub starts reporting a thread's pull request as conflicting, the ticker leaves one `pr` inbox item (`PR has conflicts`) and a notification without sound, and nothing more until GitHub reports it mergeable again. `UNKNOWN` in between changes nothing, so a conflict that GitHub recomputes is not announced twice.


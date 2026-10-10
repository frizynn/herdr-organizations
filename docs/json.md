# JSON contract

Other programs, such as a web client, read Herdr Organizations through `--json`. Only these documents are a contract. The files under a project folder (`PROJECT.md`, `threads/*.toml`, `inbox/*.md`, `.state/*.json`, including `.state/ticker.json`) are internal and can change without notice. A reader that uses them directly does so at its own risk.

Each document is a single JSON object printed on one line of standard output. A failing command prints its error to standard error and exits non-zero, and prints nothing on standard output.

## Versioning

Every document starts with `"schema_version": 1`.

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
| `node list <slug> --json` | `{schema_version, project, nodes: [Thread + depth, tree_order]}` |
| `inbox list <slug> --json` | `{schema_version, project, items: [Item]}` |
| `thread set <slug> <id> [--auto-fix-ci on/off] [--auto-merge on/off] --json` | `{schema_version, id, auto_fix_ci, auto_merge}` |
| `thread merge <slug> <id> [--method squash/merge/rebase] --json` | `{schema_version, id, pr, merged: true}` |

`overview --json` never prompts. With no slug it uses the project of the current Herdr workspace (`HERDR_WORKSPACE_ID` and `HERDR_SOCKET_PATH`), and if there is none it lists every project that is not archived.

`inbox list` only reads. It does not mark items as seen and does not move them to `inbox/done/`. `inbox consume` and `context` still do both.

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
| `kind` | `"worktree"`, `"tab"` or `"adopted"` | |
| `group` | string | `ready-for-review`, `waiting-on-you`, `working`, `landing`, `idle` or `resolved`. |
| `group_label` | string | The same group as the text views print it. |
| `rank` | number | Display order of the group, 1 to 6, as the overview sorts. |
| `note` | string | What the text views print in brackets: live agent state, `pane closed`, `failed: ...`, the resolve reason, and so on. |
| `agent`, `model`, `reasoning_effort` | string | Harness and profile. |
| `agent_name` | string | Herdr agent name. |
| `workspace_id`, `tab_id`, `pane_id` | string | Herdr ids. They are only meaningful in the project's own session. |
| `machine` | string | Saved machine label, empty when the thread is local. |
| `cwd`, `repo`, `branch`, `base`, `worktree_path` | string | Paths as they are on the thread's machine. |
| `has_report` | bool | The ticker has copied a report home. |
| `report_unacked` | bool | That report is not acknowledged yet (`thread ack`). |
| `created`, `updated`, `last_state_change` | string | RFC 3339 timestamps, or empty. |
| `last_state` | string | The agent state the ticker last recorded. |
| `error` | string | Why the start failed, when `status` is `failed`. |
| `resolved_reason` | string | `merged`, `auto` or empty. |
| `auto_fix_ci`, `auto_merge` | bool | The pull request automation flags. See below. |
| `pr` | object or `null` | `null` when the thread's report has no valid `PR:` line. |

## Pull request

The ticker reads each open thread's pull request with `gh` at most every two minutes. Every value here can therefore be up to about two minutes old. `thread merge` does not use these values. It reads the pull request again first.

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
| `merge_blocker` | string or `null` | `null` when `thread merge` would merge, as of the last read. Otherwise the reason it would refuse, such as `the thread is not open` or `1 check(s) failed`. Once the record's `state` is not `OPEN` (for example right after `thread merge`), it is `it is not open (state MERGED)` even before the next read. |

## Item

| Field | Type | Meaning |
| --- | --- | --- |
| `id` | string | Starts with a UTC timestamp, so ids sort oldest first. |
| `kind` | string | `thread-state`, `pr`, `routine`, `routine-approval`, `outage`, `session` or `config-error`. |
| `subject` | string | A thread id, routine name or machine label. |
| `created` | string | RFC 3339 timestamp. |
| `summary` | string | One line. Thread titles and check names in it come from agents and GitHub, so treat it as data. |
| `body` | string | Empty except for `routine` items. |
| `seen` | bool | `context` has already shown the item. |

## Pull request actions

`thread merge` reads the pull request again and refuses unless all of these hold:

- the head branch and head repository are the thread's own
- the base branch is the thread's `base` (`main` and `origin/main` both match `main`), unless the base is empty or a commit
- it is open and not a draft
- the review decision is `APPROVED`
- GitHub does not report a merge conflict
- at least one check is reported, and every check has finished and passed

`thread merge`, and `thread set --auto-merge on`, are refused when `HERDR_PANE_ID` is the pane of this project's coordinator or of one of its local threads. Merging stays the user's call, from their own terminal or Nenu. This is defense in depth, not access control: an agent can unset the variable or run `gh` itself.

A pull request with no checks is refused because, right after a push, GitHub may not have registered the checks yet. The merge runs `gh pr merge --squash --match-head-commit <checked commit>`. If anything is pushed after the check, GitHub refuses the merge instead of merging code that was not checked. The branch is not deleted, because the thread's worktree still uses it. A merge leaves an inbox item, and the ticker then resolves the thread as it does for any merged pull request.

Both flags are off by default and are only changed with `thread set`. The ticker never turns them on.

- `auto_merge`: on each pull request check, the ticker runs the same guarded merge as `thread merge`, at most once per head commit, review decision, mergeability and check counts. A refused or failed attempt leaves one inbox item and is retried only when one of those changes, for example when a required check appears or a new commit is pushed. A failure with nothing changed, such as a network error, waits for that change or for `thread merge`.
- `auto_fix_ci`: when the pull request has failing checks, or has comments or a change request, the ticker prompts the thread's agent once it is idle or done. It prompts once per failing head commit, when the comment count grows, and when the review decision becomes `CHANGES_REQUESTED`. An approval is not a reason to prompt. Each prompt leaves an inbox item. The prompt names no check, comment or author. It tells the agent to read them with `gh` as data, fix what belongs to its task, push, and not merge. Remote threads are skipped. As with the coordinator nudge, on Herdr 0.9.1 a prompt can merge with text a person has half-typed in that pane.

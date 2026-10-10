# Manual validation

Automated tree, hierarchy, context, profile, CLI and scenario checks run with `cargo test --locked`. The following checks require a real Herdr client and installed agent CLIs. Use a disposable session and project root.

## Prepare a disposable session

```sh
scripts/dev-server
scripts/dev-hp --root "$PWD/.dev-root" new demo
scripts/dev-hp --root "$PWD/.dev-root" open demo --session hp-dev
```

Use a scratch Git repository for worktree checks. Do not point these checks at a personal project root or default Herdr session.

## Recursive node lifecycle

1. Create a coordinator directly under root:

   ```sh
   node start demo --parent root --role coordinator --title "Area lead" --task-file - <<'TASK'
   Coordinate the demo organization and create a worker child.
   TASK
   ```

2. In the resulting pane, verify its brief contains the root-to-node instructions, the exact CLI prefix, its node id and the instruction to create children beneath itself.
3. Create a worker and another coordinator beneath that coordinator. Create a worker under the second coordinator. Confirm the tree is at least four levels deep including root.
4. Run `node list demo`. Confirm rows include role, parent, state and stable preorder. Restart one child, resolve another, and verify only that node's state changes.
5. Attempt `node start demo --parent <worker-id> ...`. Confirm the CLI rejects it and no record, task or node scope is created.
6. Close a node pane and use `node restart`. Confirm it reuses the recorded worktree or tab and preserves its profile and ancestor context.

## Scoped instructions and memory

In the disposable project, add unique sentinel text to the project root, an ancestor node, a sibling node and a descendant node. Create a child under the ancestor. Confirm its brief contains the root, ancestor and its own sentinels in that order. Confirm the sibling and descendant sentinels are absent. Repeat for `MEMORY.md` and sorted `memory/*.md` files.

## Agent profiles

Use the installed Codex CLI to check a node with a model, reasoning effort and each permission profile. Check Claude Code with model and permission profiles. Inspect Herdr's `agent start` argv or each CLI's reported startup options. Verify raw argument values containing spaces, quotes and shell metacharacters arrive as single data values and execute nothing in the shell. Confirm conflicting Codex and Claude permission flags in raw and project safety args fail before placement, and confirm permission profiles are argv validation rather than OS isolation.

## Organizations popup

Open **Herdr Organizations: launcher** (or the bound key) in a disposable session.

1. Confirm needs-you rows list first, oldest first, then each project with its coordinators numbered 1-9. Press a digit and confirm the tree opens on that coordinator.
2. In the tree, move with Up/Down and press Enter on a live node: the pane receives focus and the popup closes. Close an active tab and confirm Enter recreates it. Space folds a coordinator and the fold survives a state-file change.
3. Press `b` for the board; Left/Right/Up/Down move between cards, `f` cycles the coordinator filter, Enter opens the detail, Esc returns.
4. On a thread in review with a pull request, press `m`: the confirmation is drawn inside the popup. With a blocker it only offers Esc. Merge only in a scratch repository.
5. Press `n`: type a name and confirm the inline validation and the CLI preview update. Esc cancels without creating anything.
6. Press `s`: change dock side and notification choice with Left/Right, rebind a command (Enter, then a key) and confirm `~/.config/herdr-projects/tui.toml` holds only the changed key. A conflicting key is refused with a message.
7. Press Esc until the popup closes and confirm no `herdr-organizations pane ui` process remains.
8. Click a row to select it and click it again to open it when the client forwards mouse events.

## Native sidebar tokens and headers

1. Run `herdr-organizations sidebar install --config <disposable config>` and reload the config. Confirm a coordinator workspace shows a second row with its counts and agent rows show `$org_task` and "needs you".
2. With tokens reported, click every header: `new` creates a workspace, `menu` opens the menu, the sort word right of `agents` cycles the sort, and `spaces` and `agents` behave as they do with Herdr's default layout. Click a workspace row and an agent row and confirm focus moves.
3. Run **focus sidebar on this project** and confirm the `agents` header stays visible next to the short project label. `unfocus` clears it.

## Dock

1. Turn the dock on in Settings, then run **Herdr Organizations: toggle dock** from a project workspace. Confirm one split appears on the chosen side and width.
2. Change a thread's state and confirm the dock redraws without a key press. Confirm the dock process makes no Herdr calls while idle.
3. Toggle again and confirm only the dock pane closes. Open an unrelated pane with a similar label and confirm the toggle never closes it.

## Existing behavior

- Open a project, start a root worker with `thread start`, and confirm it is a worker child of `root`.
- Review a completed report in the project, inspect inbox and routine behavior, then run the existing `overview`, `focus` and `unfocus` actions.
- Start one remote node on a saved machine and verify its worktree, agent and report follow the existing SSH flow.
- Confirm a remote worker starts successfully and a remote coordinator with child-spawn permission is refused before a record or worktree is created.
- Confirm the old `herdr-projects` binary alias and existing `~/.herdr-projects` and `~/.config/herdr-projects` data locations still work.

Record the Herdr client and server versions, operating system, harness versions, and any client-only limitations with results.

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

Open **Herdr Organizations: organization tree** from Herdr's action menu.

1. Select a project with keyboard arrows, `j` or `k`, and press Enter.
2. Confirm root and every active descendant render in the same stable order as `node list`, with title, state, role and id. Confirm resolved leaves are absent.
3. Move through root and children using Up/Down and `j`/`k`. Press Enter on a live child and confirm the selected pane receives focus and the popup closes. Repeat for root.
4. Close an active tab, reopen the popup, select that node and press Enter. Confirm a replacement tab is created, focused and the popup closes.
5. Press Esc to return to project selection, then `q` to close.
6. Click a row to select it. Double-click to open or focus when Herdr forwards mouse events. If no mouse event reaches the popup, keyboard support remains available.
7. Press `r` after creating a node and confirm the tree refreshes.

## Contextual organization sidebar

1. From a coordinator or worker pane in a project workspace, run **Herdr Organizations: toggle project hierarchy sidebar**. Confirm one split appears on the configured side at the configured width and is scoped to that project.
2. Navigate with Up/Down and `j`/`k`. Press Space on a coordinator and confirm only its descendants fold. Press Enter on root and a live node; confirm the pane focuses and the sidebar remains available. Close a node tab and confirm Enter recreates an active node.
3. Open the visible gear with `s`. Change dock side and width, close and reopen, and confirm the layout updates. Toggle focus-on-open, auto-open, resolved nodes, status, role and strict toggle; reopen the settings and confirm values persist in Herdr's plugin config directory.
4. Confirm the settings are grouped under Layout, Behavior and Tree, values align consistently, the selected row uses a leading marker, and changing a setting does not change Herdr's global keybindings.
5. Open an unrelated pane in the same workspace and give it a similar label. Toggle the organization sidebar and confirm only the pane with matching organization sidebar, project and workspace tokens closes. Repeat with another project's workspace.
6. With auto-open enabled, focus a project tab and confirm the sidebar is ensured once. With it disabled, focus the tab and confirm no split is created.
7. Close and reopen the sidebar repeatedly from a warm release build. Confirm the first toggle adopts any existing sidebar, later toggles do not scan the workspace, the tree paints before identity and live-status hydration, and the UI remains interactive without flashing, clearing or repainting unchanged rows.

## Existing behavior

- Open a project, start a root worker with `thread start`, and confirm it is a worker child of `root`.
- Review a completed report in the project, inspect inbox and routine behavior, then run the existing `overview`, `focus` and `unfocus` actions.
- Start one remote node on a saved machine and verify its worktree, agent and report follow the existing SSH flow.
- Confirm a remote worker starts successfully and a remote coordinator with child-spawn permission is refused before a record or worktree is created.
- Confirm the old `herdr-projects` binary alias and existing `~/.herdr-projects` and `~/.config/herdr-projects` data locations still work.

Record the Herdr client and server versions, operating system, harness versions, and any client-only limitations with results.

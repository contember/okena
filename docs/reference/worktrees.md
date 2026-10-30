# Git Worktree Management

Okena has built-in support for git worktrees, letting you work on multiple branches of a repository simultaneously without switching branches or cloning the repo again. Each worktree gets its own project in the sidebar with independent terminal sessions.

## Overview

A git worktree is a linked checkout of a branch in a separate directory, sharing the same `.git` history as the main repo. Okena treats each worktree as a child project of the repository it was created from. When you create a worktree, Okena:

1. Runs `git worktree add` to create the checkout
2. Adds a new project in the sidebar linked to the parent
3. Inherits the parent project's folder color and settings
4. Automatically cleans up when the worktree directory is removed

Worktrees created outside Okena can show up on their own too, once you opt in; see [below](#worktrees-created-outside-okena).

## Creating Worktrees

Right-click a project in the sidebar and select **Create Worktree**, or use the command palette. The worktree dialog has two modes:

### From Branch

- Search or scroll through available branches (branches not already checked out in another worktree)
- Select an existing branch, or type a new branch name to create one
- Use arrow keys to navigate the branch list while the search input is focused
- Press Enter to confirm

### From PR

- Switch to the **From PR** tab to list open pull requests from GitHub (requires the `gh` CLI)
- Select a PR to check out its branch as a worktree

### Custom Path

Check **Use custom path** to override where the worktree is created on disk. The field is pre-filled with the auto-generated path based on your path template.

### From the CLI

`okena worktree add <project> <branch> [--new-branch]` returns as soon as the
row exists (`pending: true`) and checks out in the background; a failed checkout
removes the row again and shows a toast.

- `--wait` returns once the checkout exists and its terminals run, and prints
  the project id, path and first terminal id. A failed checkout exits 1 with
  Git's reason. The `create_worktree` action takes it as `wait: true` and then
  replies with `pending: false`, `terminal_id` and `terminal_ids` (the layout's
  terminals). An `on_worktree_create` hook may still be running in its own
  terminal, which `terminal_ids` leaves out. With `--window`, the worktree moves
  to that window only after the reply, and stays in the parent's window if the
  caller disconnects first.
- `--layout single` starts the worktree with one terminal; the default
  (`parent`) clones the parent project's layout with fresh terminals.
- `--name <label>` names the worktree's first terminal (`terminal_name`).

## Worktrees Created Outside Okena

The daemon keeps the sidebar in step with what Git knows about. Two background
scans do this:

- **Discovery** (every **10 seconds**, opt-in) adds linked worktrees that were created
  outside Okena (from a terminal, by a script, by Claude Code's `claude -w`)
  under their parent project. It opens every parent repository with gix and
  reads its worktree list, so a pass spawns no processes. A discovered
  worktree gets no terminal: its row shows **Start Terminal** until you open
  one, so turning discovery on in a repository with many worktrees doesn't
  start a shell in each.
- **Stale sweep** (every **30 seconds**) removes worktree projects whose
  checkout directory no longer exists (`git worktree remove` from a terminal,
  `rm -rf`, a cleanup script).

Discovery skips:

- worktrees matching `worktree.auto_track_ignore`. The default ignores
  `.claude/worktrees/agent-*`, the short-lived worktrees Claude Code gives each
  isolated subagent: a single session can create dozens, and Claude removes
  them itself when the subagent leaves no changes. Session worktrees
  (`.claude/worktrees/<name>`) are tracked.
- worktrees you untracked. Removing a worktree row from the sidebar (or
  unticking it in **Manage Worktrees**) while its checkout stays on disk is
  remembered in `workspace.json` (`dismissed_worktree_paths`), so the next
  scan doesn't add it back. Ticking it again forgets the dismissal, and so does
  deleting the checkout.
- worktrees in a detached HEAD state, and repositories reached only through a
  project that is itself a linked worktree (it would file the main checkout
  and its siblings under the wrong parent).

In a monorepo with several projects in one repository, a discovered worktree
goes under the first of them in sidebar order.

Discovery is **off by default**. Turn it on with **Settings > Worktree >
Discovery** (`auto_track`). The stale sweep always runs.

## Path Templates

The path template controls where new worktrees are created on disk. Configure it in **Settings > Worktree > Path Template**.

**Default:** `../{repo}-wt/{branch}`

### Variables

| Variable | Expands to | Example |
|----------|-----------|---------|
| `{repo}` | Repository folder name | `myproject` |
| `{branch}` | Branch name (slashes replaced with dashes) | `feature-login` |

### Examples

Given a repo at `/projects/myapp` and branch `feature/auth`:

| Template | Worktree path |
|----------|--------------|
| `../{repo}-wt/{branch}` | `/projects/myapp-wt/feature-auth` |
| `../{repo}/{branch}` | `/projects/myapp/feature-auth` |
| `/tmp/worktrees/{repo}/{branch}` | `/tmp/worktrees/myapp/feature-auth` |

Relative templates are resolved from the git repository root. Absolute templates are used as-is.

Branch names containing `/` are converted to `-` in the path (e.g., `feature/auth` becomes `feature-auth`).

## Sidebar Integration

Worktree projects appear nested under their parent project in the sidebar:

- **Parent projects** with active worktrees display a worktree count badge (branch icon + number)
- **Worktree children** are rendered indented below the parent, visually grouped together
- Worktrees **inherit the folder color** of their parent project
- Focusing any worktree child also keeps its sibling worktrees and the parent visible in the sidebar
- Worktrees inside folders appear alongside their parent within that folder

Each worktree project is fully independent: it has its own terminal sessions, layout, and hooks configuration. You can expand/collapse, rename, and interact with worktree projects the same way as regular projects.

## Closing Worktrees

Right-click a worktree project and select **Close Worktree** to open the close dialog. The dialog checks the worktree state and offers several options:

| Option | Description | Default |
|--------|-------------|---------|
| **Stash changes** | Stashes uncommitted changes before merging | Off |
| **Fetch remote** | Runs `git fetch --all` before rebasing | On |
| **Merge into default branch** | Checks out the default branch (main/master) in the main repo and merges the worktree's branch | Off |
| **Push target branch** | Pushes the default branch after merging | Off |
| **Delete branch** | Deletes the local and remote branch after merging | Off |

The close workflow runs these steps in order:

1. Stash changes (if enabled and worktree is dirty)
2. Fetch from remotes (if enabled)
3. Rebase onto default branch in the worktree (if merge enabled)
4. Checkout default branch in main repo and merge (if merge enabled)
5. Push the default branch (if enabled)
6. Delete the local and remote branch (if enabled)
7. Run the `worktree_closing` hook (if configured)
8. Remove the worktree directory and prune git metadata
9. Run the `worktree_removed` hook (if configured)

If any step fails, the operation stops and displays an error. Failed rebases are automatically aborted. Failed stashes are popped to restore the working state.

Worktree removal uses a fast path: it deletes the directory directly and runs `git worktree prune`, which is significantly faster than `git worktree remove` (which performs expensive status checks).

## Monorepo Support

Okena handles monorepos where the project directory is a subdirectory of the git repository root. When creating a worktree from a monorepo project:

- Okena detects the git root via `git rev-parse --show-toplevel`
- The worktree is created at the **repository root level** (not the subdirectory)
- The project path is set to the **same subdirectory** within the new worktree

For example, if your project is at `/repos/monorepo/packages/app`:
- Git root: `/repos/monorepo`
- Subdirectory: `packages/app`
- Worktree checkout: `/repos/monorepo-wt/feature-branch`
- Project path: `/repos/monorepo-wt/feature-branch/packages/app`

This ensures the full repository is checked out in the worktree while your project opens in the correct subdirectory.

## Configuration

Worktree settings live in `settings.json` in your platform's config dir (macOS: `~/Library/Application Support/okena/`, Linux: `~/.config/okena/`) under the `worktree` key:

```json
{
  "worktree": {
    "path_template": "../{repo}-wt/{branch}",
    "default_merge": false,
    "default_stash": false,
    "default_fetch": true,
    "default_push": false,
    "default_delete_branch": false,
    "auto_track": false,
    "auto_track_ignore": [".claude/worktrees/agent-*"]
  }
}
```

| Setting | Type | Default | Description |
|---------|------|---------|-------------|
| `path_template` | string | `../{repo}-wt/{branch}` | Path template for new worktrees |
| `default_merge` | bool | `false` | Enable merge on close by default |
| `default_stash` | bool | `false` | Stash changes before merge by default |
| `default_fetch` | bool | `true` | Fetch remotes before rebase by default |
| `default_push` | bool | `false` | Push after merge by default |
| `default_delete_branch` | bool | `false` | Delete branch after merge by default |
| `auto_track` | bool | `false` | Opt-in. Add worktrees created outside Okena to the sidebar |
| `auto_track_ignore` | string[] | `[".claude/worktrees/agent-*"]` | Globs discovery leaves alone, over the checkout path relative to the repository root (or the absolute path for a checkout outside it). `*` stays within one path segment, `**` spans several. |

You can also configure these defaults from **Settings > Worktree > Close Defaults** in the UI.

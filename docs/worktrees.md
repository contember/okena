# Git Worktree Management

Okena has built-in support for git worktrees, letting you work on multiple branches of a repository simultaneously without switching branches or cloning the repo again. Each worktree gets its own project in the sidebar with independent terminal sessions.

## Overview

A git worktree is a linked checkout of a branch in a separate directory, sharing the same `.git` history as the main repo. Okena treats each worktree as a child project of the repository it was created from. When you create a worktree, Okena:

1. Runs `git worktree add` to create the checkout
2. Adds a new project in the sidebar linked to the parent
3. Inherits the parent project's folder color and settings
4. Automatically cleans up when the worktree directory is removed

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

## Worktree Sync Watcher

Okena runs a background watcher that polls every **30 seconds** to:

- **Discover new worktrees** -- If you create a worktree from the command line (outside Okena), the watcher detects it via `git worktree list` and adds it to the sidebar automatically.
- **Remove stale worktrees** -- If a worktree directory no longer exists on disk (deleted externally), the watcher removes the corresponding project from the sidebar.

The watcher only scans non-remote, non-worktree projects (i.e., your "parent" repositories). Discovery uses canonical path comparison to avoid duplicates.

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

Worktree settings live in `~/.config/okena/settings.json` under the `worktree` key:

```json
{
  "worktree": {
    "path_template": "../{repo}-wt/{branch}",
    "default_merge": false,
    "default_stash": false,
    "default_fetch": true,
    "default_push": false,
    "default_delete_branch": false
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

You can also configure these defaults from **Settings > Worktree > Close Defaults** in the UI.

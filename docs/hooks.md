# Lifecycle Hooks

Okena can run shell commands automatically in response to project and worktree events. Hooks let you automate tasks like installing dependencies, running linters, notifying services, or launching AI agents to resolve conflicts.

## Configuration

Hooks are configured in two places:

- **Global** -- `~/.config/okena/settings.json` under the `"hooks"` key. Applies to all projects.
- **Per-project** -- stored in `workspace.json` on each project entry. Overrides the global default when set.

Per-project hooks take priority. If a project does not define a given hook, the global value is used. If neither is set, the hook does not fire.

### Global hooks (settings.json)

```json
{
  "hooks": {
    "on_project_open": "echo opened",
    "on_worktree_create": "npm install",
    "pre_merge": "./scripts/lint.sh"
  }
}
```

### Per-project hooks

Per-project hooks are set through the project settings UI and persisted in `workspace.json`. They follow the same key names as the global hooks.

## Available Hooks

| Hook | Timing | Sync/Async | Description |
|------|--------|------------|-------------|
| `on_project_open` | After a project is opened | Async | Run setup tasks when a project loads. |
| `on_project_close` | When a project is removed | Async (headless) | Cleanup when a project is deleted. No PTY terminal is created since the project is being removed. |
| `on_worktree_create` | After a worktree is created | Async | Install deps, configure environment for a new worktree. |
| `on_worktree_close` | After a worktree is removed | Async (headless) | Cleanup after worktree deletion. Runs headlessly. |
| `pre_merge` | Before a merge operation | **Sync** | Runs before merge and blocks until complete. If the hook exits non-zero, the merge is aborted. |
| `post_merge` | After a successful merge | Async | Run post-merge tasks (notifications, CI triggers). |
| `before_worktree_remove` | Before a worktree is deleted | **Sync** | Runs before removal. Non-zero exit aborts the removal. |
| `worktree_removed` | After a worktree is deleted | Async | Post-removal cleanup (e.g., delete remote branch). |
| `on_rebase_conflict` | When a rebase encounters conflicts | Async | React to rebase conflicts. Supports `terminal:` prefix. |
| `on_dirty_worktree_close` | When closing a worktree with uncommitted changes | Async | Handle dirty worktree state. Supports `terminal:` prefix. |

**Sync hooks** block the operation until the command completes. If they exit non-zero, the operation is aborted and an error is shown.

**Async hooks** run in the background and do not block the UI. Failures produce a toast notification.

**Headless hooks** run without a visible terminal (via `sh -c` on Unix, `cmd /C` on Windows). These are used for hooks that fire during teardown when creating a PTY terminal is not practical.

## Hook Terminals

By default, hook commands run as background PTY terminals visible in the service panel. For hooks that support multi-line commands (`on_rebase_conflict`, `on_dirty_worktree_close`), you can use the `terminal:` prefix to spawn a command in a new interactive terminal pane:

```
terminal: claude -p "Fix the rebase conflicts in this repo"
```

Multi-line hook commands are split by newline. Each line is an independent action:

```
echo "logging something"
terminal: claude -p "resolve conflicts"
terminal: htop
```

- Lines without a prefix run as background commands.
- Lines with `terminal:` open a new visible terminal pane with the command.

Empty lines and leading/trailing whitespace are ignored.

## Environment Variables

Hooks receive context through environment variables. The working directory is set to the project path.

### Base variables (all hooks)

| Variable | Description |
|----------|-------------|
| `OKENA_PROJECT_ID` | Unique ID of the project |
| `OKENA_PROJECT_NAME` | Display name of the project |
| `OKENA_PROJECT_PATH` | Filesystem path to the project |
| `OKENA_FOLDER_ID` | ID of the folder containing the project (if any) |
| `OKENA_FOLDER_NAME` | Display name of the folder (if any) |

### Worktree and branch variables

Available on worktree and merge hooks.

| Variable | Description |
|----------|-------------|
| `OKENA_BRANCH` | Current branch name |
| `OKENA_TARGET_BRANCH` | Target branch for merge operations (`pre_merge`, `post_merge`, `on_rebase_conflict`) |
| `OKENA_MAIN_REPO_PATH` | Path to the main repository (merge and worktree-remove hooks) |

### Terminal variables

| Variable | Description |
|----------|-------------|
| `OKENA_TERMINAL_ID` | Unique ID of the terminal (`terminal.on_close` only) |
| `OKENA_TERMINAL_NAME` | Custom name of the terminal, if set (`terminal.on_close` only) |
| `OKENA_EXIT_CODE` | Exit code of the terminal process (`terminal.on_close` only) |

### Conflict variables

| Variable | Description |
|----------|-------------|
| `OKENA_REBASE_ERROR` | Error message from a failed rebase (`on_rebase_conflict` only) |

### Variable availability by hook

| Hook | `PROJECT_*` | `FOLDER_*` | `BRANCH` | `TARGET_BRANCH` | `MAIN_REPO_PATH` | `REBASE_ERROR` | `TERMINAL_ID` | `TERMINAL_NAME` | `EXIT_CODE` |
|------|-------------|------------|----------|-----------------|-------------------|----------------|---------------|-----------------|-------------|
| `on_project_open` | yes | if in folder | | | | | | | |
| `on_project_close` | yes | if in folder | | | | | | | |
| `on_worktree_create` | yes | if in folder | yes | | | | | | |
| `on_worktree_close` | yes | if in folder | yes | | | | | | |
| `pre_merge` | yes | if in folder | yes | yes | yes | | | | |
| `post_merge` | yes | if in folder | yes | yes | yes | | | | |
| `before_worktree_remove` | yes | if in folder | yes | | yes | | | | |
| `worktree_removed` | yes | if in folder | yes | | yes | | | | |
| `on_rebase_conflict` | yes | if in folder | yes | yes | yes | yes | | | |
| `on_dirty_worktree_close` | yes | if in folder | yes | | | | | | |
| `terminal.on_create` | yes | if in folder | worktree | | | | | | |
| `terminal.shell_wrapper` | yes | if in folder | worktree | | | | | | |
| `terminal.on_close` | yes | if in folder | worktree | | | | yes | if set | yes |

For `terminal.on_create` and `terminal.shell_wrapper`, environment variables are exported into the shell session so they persist after the hook command runs. For worktree projects, `OKENA_BRANCH` is included automatically.

## Hook Monitor

Okena tracks the last 50 hook executions in the hook monitor. Each execution records:

- Hook type and command
- Project name
- Start time and duration
- Status: Running, Succeeded, Failed (with exit code and stderr), or SpawnError
- Associated terminal ID (when using PTY execution)

When a hook fails or cannot start, Okena shows a toast notification with the first line of stderr (truncated to 120 characters).

## Examples

### Auto-install dependencies on worktree creation

```json
{
  "hooks": {
    "on_worktree_create": "npm install"
  }
}
```

### Run linting before merge

```json
{
  "hooks": {
    "pre_merge": "./scripts/lint.sh"
  }
}
```

Since `pre_merge` is synchronous, the merge will be aborted if linting fails.

### Launch Claude to resolve rebase conflicts

```json
{
  "hooks": {
    "on_rebase_conflict": "terminal: claude -p \"There are rebase conflicts. Please resolve them.\""
  }
}
```

This opens an interactive terminal pane running Claude with a prompt to fix the conflicts.

### Cleanup after worktree removal

```json
{
  "hooks": {
    "worktree_removed": "git push origin --delete $OKENA_BRANCH"
  }
}
```

### Handle dirty worktree close with stash and notification

```json
{
  "hooks": {
    "on_dirty_worktree_close": "git stash push -m \"auto-stash on close: $OKENA_BRANCH\"\nterminal: echo 'Changes stashed for branch $OKENA_BRANCH'"
  }
}
```

The first line stashes changes in the background. The second line opens a terminal showing the confirmation.

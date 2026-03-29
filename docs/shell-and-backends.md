# Shell Selector and Session Backends

## Shell Selector

Okena supports multiple shells and lets you configure the default globally, per-project, or per-terminal.

### Supported Shells

**Linux / macOS:**
- System default
- Bash (`/bin/bash`)
- Zsh (`/bin/zsh`)
- Fish (`/bin/fish`)
- sh (`/bin/sh`)
- Custom (any command with arguments)

**Windows:**
- Command Prompt (`cmd.exe`)
- Windows PowerShell (`powershell.exe`)
- PowerShell Core (`pwsh.exe`)
- WSL (with distro selection)
- Custom

### Configuration

**Global default** in `settings.json`:

```json
{
  "default_shell": "Default"
}
```

Values: `"Default"`, `"Bash"`, `"Zsh"`, `"Fish"`, or a custom object:

```json
{
  "default_shell": { "Custom": { "path": "/usr/local/bin/nu", "args": [] } }
}
```

**Per-project override:** Set through the project settings UI. When set, all new terminals in that project use the project shell instead of the global default.

**Shell selector UI:** Enable `"show_shell_selector": true` in settings to show a shell picker in the terminal header.

### Shell Resolution Order

1. Terminal's own shell type (if explicitly set)
2. Project's default shell (if set)
3. Global default shell (if set)
4. System default shell

## WSL Support (Windows)

Okena integrates with Windows Subsystem for Linux. Each installed WSL distribution appears as a separate shell option.

### How It Works

- Okena runs `wsl.exe -l -q` to discover installed distributions
- Windows paths are automatically converted to WSL mount points (`C:\Users` -> `/mnt/c/Users`)
- Session backends (tmux/dtach) are resolved independently per distribution
- Each WSL terminal runs `wsl.exe -d <distro> -- sh -c "<command>"`

### Configuration

Select a WSL shell in settings:

```json
{
  "default_shell": { "Wsl": { "distro": "Ubuntu" } }
}
```

Omit `distro` (or set to `null`) to use the default WSL distribution.

## Session Backends

Session backends provide terminal persistence -- your terminal sessions survive app restarts.

### Available Backends

| Backend | Persistence | Description |
|---------|-------------|-------------|
| **None** | No | Direct PTY -- sessions end when the app closes |
| **Dtach** | Yes | Lightweight Unix socket persistence |
| **Tmux** | Yes | Full-featured terminal multiplexer |
| **Screen** | Yes | GNU Screen sessions |
| **Auto** | Yes | Auto-detect best available (prefers dtach > tmux > screen) |

### Configuration

```json
{
  "session_backend": "Auto"
}
```

Or via environment variable: `OKENA_SESSION_BACKEND` (values: `auto`, `tmux`, `screen`, `dtach`, `none`).

### How Persistence Works

With a persistent backend:

1. On terminal creation, Okena starts a session (e.g., `dtach -A <socket> <shell>`)
2. The terminal ID is saved in `workspace.json`
3. On app restart, Okena reconnects to the existing session
4. Terminal output from while the app was closed is preserved (backend-dependent)

### Backend Comparison

| Feature | Dtach | Tmux | Screen |
|---------|-------|------|--------|
| Overhead | Minimal | Moderate | Moderate |
| Scrollback | Shell-native | Tmux-managed (2000 lines) | Screen-managed |
| Socket location | `$XDG_RUNTIME_DIR/okena/` or `/tmp/okena-<uid>/` | N/A | N/A |
| Buffer capture | No | Yes (`tmux capture-pane`) | No |
| Recommended for | Most users | Advanced users needing buffer capture | Legacy systems |

### Shell Wrapper Hook

Wrap the shell command for custom execution contexts (e.g., dev containers):

```json
{
  "hooks": {
    "terminal": {
      "shell_wrapper": "devcontainer exec --workspace-folder $OKENA_PROJECT_PATH -- {shell}"
    }
  }
}
```

The `{shell}` placeholder is replaced with the resolved shell command.

### Stale Session Cleanup

On startup, Okena cleans up dtach sockets that have no active listeners, preventing accumulation of dead sessions.

## Environment Variables

Okena sets these variables in every terminal:

| Variable | Value |
|----------|-------|
| `TERM` | `xterm-256color` |
| `COLORTERM` | `truecolor` |
| `OKENA_TERMINAL_ID` | Terminal's UUID |
| `OKENA_SESSION_BACKEND` | Resolved backend name |

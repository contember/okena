# Okena Configuration Guide

Okena stores all configuration files in your system's config directory:

| Platform | Path |
|----------|------|
| macOS | `~/.config/okena/` |
| Linux | `~/.config/okena/` |
| Windows | `%APPDATA%\okena\` |

The directory contains:

```
~/.config/okena/
  settings.json        # App settings (fonts, theme, shell, etc.)
  keybindings.json     # Custom keyboard shortcuts
  workspace.json       # Project layouts and terminal state (auto-managed)
  themes/              # Custom theme JSON files
    example-theme.json
```

---

## settings.json

The main configuration file. Okena creates it with defaults on first launch. You can edit it by hand or use the in-app settings panel (`Cmd+,` / `Ctrl+,`). To open the raw file, press `Cmd+Alt+,` / `Ctrl+Alt+,`.

If the file contains invalid JSON, Okena recovers as many fields as possible and falls back to defaults for the rest.

### Full Example

```json
{
  "version": 1,
  "theme_mode": "Dark",
  "font_family": "JetBrains Mono",
  "font_size": 14.0,
  "ui_font_size": 13.0,
  "file_font_size": 12.0,
  "line_height": 1.3,
  "cursor_style": "Block",
  "cursor_blink": false,
  "scrollback_lines": 10000,
  "default_shell": "Default",
  "show_shell_selector": false,
  "session_backend": "Auto",
  "file_opener": "",
  "show_focused_border": false,
  "sidebar": {
    "is_open": false,
    "auto_hide": false,
    "width": 250
  },
  "remote_server_enabled": false,
  "remote_listen_address": "127.0.0.1",
  "claude_code_integration": false,
  "codex_integration": false,
  "auto_update_enabled": true,
  "diff_view_mode": "Unified",
  "diff_ignore_whitespace": false,
  "idle_timeout_secs": 0,
  "min_column_width": 400,
  "hooks": {},
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

### Settings Reference

#### Appearance

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `theme_mode` | string | `"Auto"` | Theme to use. Values: `"Auto"`, `"Dark"`, `"Light"`, `"PastelDark"`, `"HighContrast"`, `"Custom"` |
| `font_family` | string | `"JetBrains Mono"` | Terminal font family |
| `font_size` | float | `14.0` | Terminal font size (8.0 - 48.0) |
| `ui_font_size` | float | `13.0` | Font size for panels and dialogs (8.0 - 24.0) |
| `file_font_size` | float | `12.0` | Font size for file/diff viewer (8.0 - 24.0) |
| `line_height` | float | `1.3` | Line height multiplier (1.0 - 3.0) |
| `show_focused_border` | bool | `false` | Show a border around the focused terminal |

#### Terminal

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `cursor_style` | string | `"Block"` | Cursor shape: `"Block"`, `"Bar"`, or `"Underline"` |
| `cursor_blink` | bool | `false` | Enable cursor blinking |
| `scrollback_lines` | int | `10000` | Number of scrollback lines (100 - 100,000) |
| `default_shell` | string | `"Default"` | Shell for new terminals. `"Default"` uses the system shell. On Linux/macOS you can also use `"Bash"`, `"Zsh"`, `"Fish"`, etc. |
| `show_shell_selector` | bool | `false` | Show shell picker in the terminal header |
| `idle_timeout_secs` | int | `0` | Seconds before a terminal is considered idle (0 = disabled) |

#### Session Backend

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `session_backend` | string | `"Auto"` | Session persistence backend. Values: `"Auto"`, `"None"`, `"Tmux"`, `"Screen"`, `"Dtach"`. Auto prefers dtach, then tmux, then screen. Not supported on Windows. |

#### Sidebar

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `sidebar.is_open` | bool | `false` | Whether the sidebar starts open |
| `sidebar.auto_hide` | bool | `false` | Auto-hide the sidebar when focus leaves it |
| `sidebar.width` | float | `250` | Sidebar width in pixels (150 - 500) |

#### File Opener

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `file_opener` | string | `""` | Editor command for opening file paths (e.g. `"code"`, `"cursor"`, `"zed"`, `"vim"`). Empty = system default |

#### Diff Viewer

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `diff_view_mode` | string | `"Unified"` | Diff display mode: `"Unified"` or `"SideBySide"` |
| `diff_ignore_whitespace` | bool | `false` | Ignore whitespace changes in diffs |

#### Remote Server

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `remote_server_enabled` | bool | `false` | Enable the HTTP/WebSocket remote control server |
| `remote_listen_address` | string | `"127.0.0.1"` | Listen address for the remote server |

#### Integrations

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `claude_code_integration` | bool | `false` | Show Claude Code status indicator in the status bar |
| `codex_integration` | bool | `false` | Show Codex status indicator in the status bar |
| `auto_update_enabled` | bool | `true` | Check for updates automatically |

#### Claude Extension Settings

The Claude extension reads credentials from `~/.claude/.credentials.json` by default. If you maintain multiple Claude Code accounts (for example, a personal account and a work account in a different directory), you can override this via `extension_settings`:

```json
{
  "extension_settings": {
    "claude-code": {
      "config_dir": "/Users/you/.claude-work"
    }
  }
}
```

Precedence (highest to lowest):
1. `extension_settings."claude-code".config_dir` — explicit path in `settings.json` (tilde `~` is expanded)
2. `CLAUDE_CONFIG_DIR` environment variable — Claude CLI's own convention
3. `$HOME/.claude` — the default

If the configured directory does not exist, Okena logs a warning and falls back to the next tier.

#### Hooks

Global lifecycle hooks run shell commands on project events. Each hook value is a shell command string (or `null` to disable).

```json
{
  "hooks": {
    "on_project_open": "echo 'Project opened'",
    "on_project_close": "echo 'Project closed'",
    "on_worktree_create": "npm install",
    "on_worktree_close": null,
    "pre_merge": "cargo test",
    "post_merge": "notify-send 'Merge complete'",
    "before_worktree_remove": null,
    "worktree_removed": null,
    "on_rebase_conflict": null,
    "on_dirty_worktree_close": null
  }
}
```

#### Worktree Defaults

Controls default behavior when creating and closing git worktrees:

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `worktree.path_template` | string | `"../{repo}-wt/{branch}"` | Path template for new worktrees. Variables: `{repo}` (repo folder name), `{branch}` (branch name) |
| `worktree.default_merge` | bool | `false` | Merge worktree branch on close |
| `worktree.default_stash` | bool | `false` | Stash changes on close |
| `worktree.default_fetch` | bool | `true` | Fetch from remote on close |
| `worktree.default_push` | bool | `false` | Push branch on close |
| `worktree.default_delete_branch` | bool | `false` | Delete branch after close |

---

## keybindings.json

Custom keybindings override the defaults. The file maps action names to an array of binding entries. Each entry has a `keystroke`, an optional `context`, and an `enabled` flag.

Okena generates this file with defaults if it does not exist. You can view and edit bindings in-app with `Cmd+K Cmd+S` / `Ctrl+K Ctrl+S`.

### Format

```json
{
  "version": 1,
  "bindings": {
    "ActionName": [
      {
        "keystroke": "cmd-d",
        "context": "TerminalPane",
        "enabled": true
      }
    ]
  }
}
```

### Keystroke Syntax

Keystrokes use modifiers joined by `-`:

- **Modifiers:** `cmd`, `ctrl`, `alt`, `shift`
- **Keys:** letters (`a`-`z`), numbers (`0`-`9`), symbols (`-`, `=`, `[`, `]`, `,`, etc.), and special keys (`escape`, `tab`, `enter`, `up`, `down`, `left`, `right`, `pageup`, `pagedown`)
- **Chords:** Two keystrokes separated by a space, e.g. `"cmd-k cmd-s"`

Examples: `"cmd-shift-d"`, `"ctrl-alt-left"`, `"shift-pageup"`, `"cmd-k cmd-t"`

### Context Scoping

Bindings without a `context` are global. Adding a context limits the binding to when that UI element is focused:

- `"TerminalPane"` -- only when a terminal pane has focus
- `"SearchBar"` -- only when the search bar has focus
- `"Sidebar"` -- only when the sidebar has focus

### Example: Custom Bindings

```json
{
  "version": 1,
  "bindings": {
    "SplitVertical": [
      { "keystroke": "cmd-d", "context": "TerminalPane" },
      { "keystroke": "ctrl-shift-d", "context": "TerminalPane" }
    ],
    "SplitHorizontal": [
      { "keystroke": "cmd-shift-d", "context": "TerminalPane" },
      { "keystroke": "ctrl-d", "context": "TerminalPane" }
    ],
    "Copy": [
      { "keystroke": "cmd-c", "context": "TerminalPane" },
      { "keystroke": "ctrl-shift-c", "context": "TerminalPane" }
    ],
    "ShowCommandPalette": [
      { "keystroke": "cmd-shift-p" },
      { "keystroke": "ctrl-shift-p" }
    ]
  }
}
```

### Disabling a Binding

Set `enabled` to `false` to disable a specific binding without removing it:

```json
{
  "bindings": {
    "ToggleSidebar": [
      { "keystroke": "cmd-b", "enabled": false },
      { "keystroke": "ctrl-b" }
    ]
  }
}
```

### Available Actions

| Action | Default Key (macOS / Linux) | Description |
|--------|----------------------------|-------------|
| `ToggleSidebar` | `Cmd+B` / `Ctrl+B` | Show or hide the sidebar |
| `ToggleSidebarAutoHide` | `Cmd+Shift+B` / `Ctrl+Shift+B` | Toggle sidebar auto-hide |
| `FocusSidebar` | `Cmd+1` / `Ctrl+1` | Focus the sidebar |
| `ClearFocus` | `Cmd+0` / `Ctrl+0` | Clear focus |
| `ShowCommandPalette` | `Cmd+Shift+P` / `Ctrl+Shift+P` | Open command palette |
| `ShowSettings` | `Cmd+,` / `Ctrl+,` | Open settings panel |
| `OpenSettingsFile` | `Cmd+Alt+,` / `Ctrl+Alt+,` | Open settings JSON file |
| `ShowKeybindings` | `Cmd+K Cmd+S` / `Ctrl+K Ctrl+S` | Show keybindings overlay |
| `ShowThemeSelector` | `Cmd+K Cmd+T` / `Ctrl+K Ctrl+T` | Open theme selector |
| `ShowSessionManager` | `Cmd+K Cmd+W` / `Ctrl+K Ctrl+W` | Open session manager |
| `ShowFileSearch` | `Cmd+P` / `Ctrl+P` | File search |
| `ShowProjectSwitcher` | `Cmd+E` / `Ctrl+E` | Switch between projects |
| `SplitVertical` | `Cmd+D` / `Ctrl+Shift+D` | Split terminal vertically |
| `SplitHorizontal` | `Cmd+Shift+D` / `Ctrl+D` | Split terminal horizontally |
| `AddTab` | `Cmd+T` / `Ctrl+Shift+T` | Add a new tab |
| `CloseTerminal` | `Cmd+W` / `Ctrl+Shift+W` | Close the focused terminal |
| `Copy` | `Cmd+C` / `Ctrl+Shift+C` | Copy selection |
| `Paste` | `Cmd+V` / `Ctrl+Shift+V` | Paste from clipboard |
| `Search` | `Cmd+F` / `Ctrl+F` | Search in terminal |
| `ScrollUp` / `ScrollDown` | `Shift+PgUp` / `Shift+PgDn` | Scroll terminal output |
| `ZoomIn` / `ZoomOut` | `Cmd+=` / `Cmd+-` | Zoom terminal font |
| `ResetZoom` | `Cmd+0` (in terminal) | Reset terminal zoom |
| `FocusNextTerminal` | `Cmd+Shift+]` / `Ctrl+Tab` | Focus next terminal |
| `FocusPrevTerminal` | `Cmd+Shift+[` / `Ctrl+Shift+Tab` | Focus previous terminal |
| `FocusLeft/Right/Up/Down` | `Cmd+Alt+Arrow` | Directional focus navigation |
| `ToggleFullscreen` | `Shift+Escape` (in terminal) | Toggle terminal fullscreen |
| `TogglePaneSwitcher` | `` Cmd+` `` / `` Ctrl+` `` | Quick pane switcher |

Okena warns on startup if it detects conflicting keybindings (same keystroke and context assigned to different actions).

---

## Custom Themes

Place custom theme JSON files in `~/.config/okena/themes/`. Okena creates this directory with an `example-theme.json` on first launch.

To activate a custom theme, set `theme_mode` to `"Custom"` in `settings.json`, then select your theme from the theme selector (`Cmd+K Cmd+T`).

### Theme File Format

```json
{
  "name": "My Theme",
  "description": "A brief description",
  "is_dark": true,
  "colors": {
    "bg_primary": "#1a1a1a",
    "bg_secondary": "#222222",
    "bg_header": "#282828",
    "text_primary": "#eeeeee",
    "text_secondary": "#999999",
    "text_muted": "#666666",
    "border": "#3a3a3a",
    "border_active": "#96cbfe",
    "border_focused": "#96cbfe",
    "cursor": "#ffa560",
    "term_foreground": "#bbbbbb",
    "term_background": "#000000",
    "term_red": "#ff6c60",
    "term_green": "#a8ff60",
    "term_yellow": "#ffffb6",
    "term_blue": "#96cbfe",
    "term_magenta": "#ff73fd",
    "term_cyan": "#c6c5fe",
    "term_white": "#eeeeee",
    "success": "#a8ff60",
    "warning": "#ffffb6",
    "error": "#ff6c60"
  }
}
```

All color fields are optional -- any field you omit falls back to the built-in dark theme default. Colors use standard hex format (`"#rrggbb"`).

### Color Categories

The full set of customizable colors includes:

- **Backgrounds:** `bg_primary`, `bg_secondary`, `bg_header`, `bg_selection`, `bg_hover`
- **Borders:** `border`, `border_active`, `border_focused`, `border_bell`, `border_idle`
- **Text:** `text_primary`, `text_secondary`, `text_muted`
- **Selection:** `selection_bg`, `selection_fg`
- **Search:** `search_match_bg`, `search_current_bg`
- **Terminal ANSI:** `term_black` through `term_white`, `term_bright_black` through `term_bright_white`, `term_foreground`, `term_background`, `term_background_unfocused`
- **UI elements:** `cursor`, `scrollbar`, `scrollbar_hover`
- **Status:** `success`, `warning`, `error`
- **Buttons:** `button_primary_bg`, `button_primary_fg`, `button_primary_hover`
- **Folders:** `folder_default`, `folder_red`, `folder_orange`, `folder_yellow`, `folder_lime`, `folder_green`, `folder_teal`, `folder_cyan`, `folder_blue`, `folder_indigo`, `folder_purple`, `folder_pink`
- **Diff:** `diff_added_bg`, `diff_removed_bg`, `diff_added_fg`, `diff_removed_fg`, `diff_hunk_header_bg`, `diff_hunk_header_fg`

---

## Per-Project Settings

Individual projects can override global hooks and the default shell. These overrides are stored in `workspace.json` as part of each project's data (managed through the UI, not typically hand-edited).

### Per-Project Hooks

Each project has its own `hooks` object that overrides the global hooks from `settings.json`. Set a hook to a command string to override, or leave it `null` to fall back to the global setting.

### Per-Project Shell

Each project can specify a `default_shell` that overrides the global `default_shell` setting. When set to `null`, the project uses the global default.

---

## workspace.json

This file stores your project list, terminal layouts, and session state. It is **auto-managed** by Okena -- you should not need to edit it by hand.

Contents include:
- Project definitions (name, path, folder color)
- Terminal layout trees (splits, tabs, terminal IDs)
- Terminal custom names and hidden/minimized state
- Project ordering and folder groupings
- Worktree metadata for git worktree projects
- Active session name

Okena auto-saves this file (debounced at 500ms) whenever project or layout state changes. A backup is created before each save. If the file becomes corrupted, Okena attempts recovery and validation on load, normalizing layouts and fixing inconsistencies.

### Sessions

You can save and restore named workspace sessions via the session manager (`Cmd+K Cmd+W` / `Ctrl+K Ctrl+W`). Sessions are exported snapshots of `workspace.json` and stored alongside it.

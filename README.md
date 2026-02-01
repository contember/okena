# Muxy

A fast, native terminal multiplexer built in Rust with [GPUI](https://github.com/zed-industries/zed/tree/main/crates/gpui) (the UI framework from Zed editor).
Tabs, splits, detachable windows, command palette, and automatic workspace restore.

## Installation

### macOS

**Homebrew (recommended):**

```bash
brew tap contember/muxy
brew install --cask muxy
```

**Or install script:**

```bash
curl -fsSL https://raw.githubusercontent.com/contember/muxy/main/install.sh | bash
```

### Linux

```bash
curl -fsSL https://raw.githubusercontent.com/contember/muxy/main/install.sh | bash
```

Installs to `~/.local/bin/muxy` with desktop entry and icons.

### Windows

**PowerShell:**

```powershell
irm https://raw.githubusercontent.com/contember/muxy/main/install.ps1 | iex
```

Installs to `%LOCALAPPDATA%\Programs\Muxy` with Start Menu shortcut.

### Manual Download

Download from the [Releases](https://github.com/contember/muxy/releases) page or get development builds:

| Platform | Download |
|----------|----------|
| macOS (Apple Silicon) | [muxy-macos-arm64.zip](https://nightly.link/contember/muxy/workflows/build/main/muxy-macos-arm64.zip) |
| Linux (x64) | [muxy-linux-x64.zip](https://nightly.link/contember/muxy/workflows/build/main/muxy-linux-x64.zip) |
| Windows (x64) | [muxy-windows-x64.zip](https://nightly.link/contember/muxy/workflows/build/main/muxy-windows-x64.zip) |

## Features

### Layout & Window Management
- **Split panes** - Horizontal and vertical splits with drag-to-resize dividers
- **Tabs** - Organize terminals in tabbed containers with reordering support
- **Detachable windows** - Pop out any terminal into a separate floating window and reattach later
- **Fullscreen mode** - Focus on a single terminal with next/previous cycling
- **Minimize/restore** - Collapse terminals to their header to save space
- **Per-terminal zoom** - Adjust zoom level (0.5x to 3.0x) independently per terminal
- **Directional focus navigation** - Move focus between panes using arrow-key shortcuts

### Multi-Project Workspace
- **Project columns** - Manage multiple projects side-by-side with resizable columns
- **Sidebar** - Collapsible project list with tree view of terminals, drag-and-drop reordering, and auto-hide mode
- **Folder colors** - Color-code projects (red, orange, yellow, green, blue, purple, pink)
- **Project switcher** - Quick searchable project navigation overlay
- **Workspace persistence** - Auto-saves full layout, terminal state, and settings to disk

### Terminal
- **Full terminal emulation** - Powered by alacritty_terminal with complete ANSI support
- **Search** - Inline text search with regex support, case sensitivity toggle, and match count
- **Link detection** - Clickable URLs and file paths (supports `file:line:col` syntax)
- **File opener integration** - Open detected files in your editor (VS Code, Cursor, Zed, Sublime, vim, etc.)
- **Configurable scrollback** - 100 to 100,000 lines
- **Cursor blink** - Toggleable cursor blinking
- **Bell notification** - Visual indicator when a terminal rings the bell
- **Per-terminal shell selection** - Choose a different shell for each terminal
- **Context menu** - Right-click for copy, paste, select all, and link actions

### Session Persistence
- **Session backends** - Keep terminals alive across app restarts using dtach, tmux, or screen (Unix)
- **Auto-detection** - Automatically selects the best available backend (dtach > tmux > screen)
- **Session manager** - Save, load, rename, and delete named workspace sessions
- **Export/import** - Export workspaces to JSON and import them back

### Git Integration
- **Git worktree support** - Create and manage git worktrees as projects directly from the UI
- **Branch detection** - Displays current branch, handles detached HEAD
- **Diff stats** - Tracks lines added/removed with cached git status

### Themes & Appearance
- **Built-in themes** - Dark, Light, Pastel Dark, and High Contrast
- **Auto theme** - Follows system light/dark appearance
- **Custom themes** - Load your own theme from a custom themes directory
- **Configurable fonts** - Font family, size (8-48pt), line height (1.0-3.0), and separate UI font size

### Command Palette & Overlays
- **Command palette** - Searchable list of all actions with keybinding hints
- **File search** - Fast file lookup within a project (respects .gitignore-style filtering)
- **Settings panel** - GUI for all preferences (theme, font, terminal, hooks, per-project settings)
- **Theme selector** - Live-preview theme picker
- **Keybindings help** - Categorized shortcut reference with search
- **File viewer** - Syntax-highlighted file preview with line numbers and search

### Customization
- **Custom keybindings** - Override any shortcut via `keybindings.json`
- **Lifecycle hooks** - Run commands on project open/close and worktree create/close (global or per-project)
- **Per-project settings** - Override global settings per project
- **Shell configuration** - Set default shell or pick per terminal (bash, zsh, fish, cmd, PowerShell, WSL)

### Platform Support
- **macOS** - Native traffic light buttons, extended PATH for homebrew shells
- **Linux** - Wayland maximize workaround, auto-detected shells
- **Windows** - Custom titlebar, cmd/PowerShell/WSL support with distro detection

### Status Bar
- CPU usage, memory usage, and current time displayed at the bottom

## Building

Requires Rust toolchain (edition 2021).

```bash
cargo build --release
```

## Running

```bash
cargo run
```

## Keyboard Shortcuts

| Action | macOS | Linux/Windows |
|--------|-------|---------------|
| New terminal | Cmd+T | Ctrl+T |
| Close terminal | Cmd+W | Ctrl+W |
| Split horizontal | Cmd+D | Ctrl+D |
| Split vertical | Cmd+Shift+D | Ctrl+Shift+D |
| Navigate panes | Cmd+Alt+Arrow | Ctrl+Alt+Arrow |
| Next/prev terminal | Cmd+Shift+]/[ | Ctrl+Tab / Ctrl+Shift+Tab |
| Fullscreen terminal | Shift+Escape | Shift+Escape |
| Command palette | Cmd+Shift+P | Ctrl+Shift+P |
| File search | Cmd+P | Ctrl+P |
| Find | Cmd+F | Ctrl+F |
| Copy | Cmd+C | Ctrl+C |
| Paste | Cmd+V | Ctrl+V |
| Zoom in/out | Cmd++/- | Ctrl++/- |
| Reset zoom | Cmd+0 | Ctrl+0 |
| Toggle sidebar | Cmd+B | Ctrl+B |
| Settings | Cmd+, | Ctrl+, |

All shortcuts are customizable via `~/.config/muxy/keybindings.json`.

## Configuration

Settings are stored in `~/.config/muxy/`:

| File | Purpose |
|------|---------|
| `settings.json` | Theme, font, shell, scrollback, hooks, and other preferences |
| `workspace.json` | Projects, layouts, and terminal state |
| `keybindings.json` | Custom keyboard shortcuts |

## Dependencies

- **GPUI** + **gpui-component** - UI framework
- **alacritty_terminal** - Terminal emulation
- **portable-pty** - PTY management
- **smol** - Async runtime

## License

MIT

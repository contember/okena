# Okena - Development Notes

Cross-platform terminal multiplexer built with Rust and GPUI (from Zed editor).

## Quick Reference

### Build Commands

**Linux:**
```bash
cargo build
cargo run
```

**Windows** (from x64 Native Tools Command Prompt for VS 2022):
```cmd
cargo build
cargo run
```

### Project Structure

```
src/
├── main.rs               # Entry point, GPUI setup, window creation
├── app.rs                # Okena - main app state, PTY event routing
├── settings.rs           # Global settings (theme, font, shell, session backend)
├── theme.rs              # ThemeMode, FolderColor, color utilities
├── assets.rs             # Embedded fonts and icons
├── simple_root.rs        # Linux Wayland maximize fix
├── terminal/             # Terminal emulation & PTY management
├── views/                # UI components
├── workspace/            # State management & persistence
├── keybindings/          # Keyboard actions & config
├── elements/             # Custom GPUI rendering (terminal grid)
├── git/                  # Git worktree integration
└── ui/                   # Shared UI utilities
```

## Architecture

### Core Modules

| Module | Key File | Purpose |
|--------|----------|---------|
| App | `app.rs` | Okena entity - owns RootView, Workspace, routes PTY events |
| Terminal | `terminal/terminal.rs` | Wraps alacritty_terminal, ANSI processing, selection, search |
| PTY | `terminal/pty_manager.rs` | PTY lifecycle, async I/O via smol + async_channel |
| Shell | `terminal/shell_config.rs` | Shell detection (bash/zsh/fish/cmd/PowerShell/WSL) |
| Session | `terminal/session_backend.rs` | tmux/screen persistence (Unix only) |
| Workspace | `workspace/state.rs` | Projects, layouts (LayoutNode tree), focus management |
| Persistence | `workspace/persistence.rs` | Load/save workspace.json, settings.json |
| Settings | `settings.rs` | Font, theme, shell prefs with debounced save |
| Keybindings | `keybindings/config.rs` | Custom keybindings from keybindings.json |

### View Hierarchy

```
RootView (views/root.rs)
├── TitleBar (chrome/title_bar.rs)
├── Sidebar (panels/sidebar.rs) - project list
├── ProjectColumns (panels/project_column.rs)
│   └── LayoutContainer (layout/layout_container.rs)
│       ├── TerminalPane (layout/terminal_pane/)
│       ├── SplitPane (layout/split_pane.rs)
│       └── Tabs
├── StatusBar (panels/status_bar.rs)
└── Overlays (overlays/)
    ├── FullscreenTerminal
    ├── CommandPalette
    ├── SettingsPanel
    ├── SessionManager
    ├── KeybindingsHelp
    └── ...
```

### Layout System

Terminals are organized in a recursive tree structure (`LayoutNode`):
- **Terminal** - single terminal pane
- **Split** - horizontal/vertical split with children
- **Tabs** - tabbed container with multiple children

Path-based navigation: `Vec<usize>` indexes into the tree.

### State Management

**GPUI Entities** (observable state with auto-notify):
- `Workspace` - projects, layouts, focus state, fullscreen
- `SettingsState` - user preferences
- `AppTheme` - current theme mode
- `RootView` - overlay states, sidebar animation

**Event Flow:**
1. PTY events: `PtyManager` → async_channel → `Okena` → `Terminal`
2. State mutations: trigger notify → observers update UI
3. Workspace changes: debounced 500ms save to disk

### Configuration Files

Located in `~/.config/okena/`:
- `workspace.json` - projects, layouts, terminal state
- `settings.json` - font, theme, shell, session backend
- `keybindings.json` - custom keyboard shortcuts

## Platform Support

### Linux
- Standard shells: bash, zsh, fish, sh (auto-detected)
- Session backends: tmux, screen (optional)
- `simple_root.rs` - Wayland maximize workaround

### Windows
- Shells: cmd, PowerShell (classic/core), WSL with distro detection
- WSL path conversion: `C:\Path` → `/mnt/c/Path`
- Custom titlebar (client-side decorations)
- Session backends not supported

### macOS
- Native traffic light buttons
- Extended PATH for homebrew shells

## Building on Windows

### Prerequisites

1. **Visual Studio 2022** with:
   - MSVC v143 - VS 2022 C++ x64/x86 build tools
   - **Windows 10/11 SDK** (e.g., `Windows 10 SDK (10.0.22621.0)`)

2. **Rust** toolchain (via rustup)

### PATH Conflict

Git for Windows includes `C:\Program Files\Git\usr\bin\link.exe` which conflicts with MSVC's `link.exe`. Errors like:
- `link: extra operand '...'`
- `Try 'link --help' for more information`

**Solution:** Build from **x64 Native Tools Command Prompt for VS 2022**.

Or use vcvars64.bat:
```cmd
call "C:\Program Files\Microsoft Visual Studio\2022\Community\VC\Auxiliary\Build\vcvars64.bat"
cargo build
```

### Common Build Errors

| Error | Cause | Solution |
|-------|-------|----------|
| `LNK1181: cannot open kernel32.lib` | Windows SDK not installed | Install via VS Installer |
| `link: extra operand` | Wrong link.exe (Git) | Use VS Developer Command Prompt |

## Key Dependencies

- **gpui** - UI framework (from Zed)
- **alacritty_terminal** - Terminal emulation
- **portable-pty** - Cross-platform PTY
- **smol** - Async runtime
- **serde/serde_json** - Serialization

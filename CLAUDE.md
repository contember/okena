# Okena - Development Notes

Cross-platform terminal multiplexer built with Rust and GPUI (from Zed editor).

Detailed module documentation lives in `src/*/CLAUDE.md` files.

## Build Commands

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

## Project Structure

```
src/
├── main.rs               # Entry point, GPUI setup, window creation
├── settings.rs           # Global settings entity (SettingsState, auto-save)
├── assets.rs             # Embedded fonts and icons
├── process.rs            # Cross-platform subprocess spawning
├── macros.rs             # Shared macros (impl_focusable!)
├── simple_root.rs        # Linux Wayland maximize workaround
├── app/                  # Main app entity, PTY event routing
├── terminal/             # Terminal emulation & PTY management
├── workspace/            # State management & persistence
├── views/                # UI views (root, layout, panels, overlays, components)
├── elements/             # Custom GPUI rendering (terminal grid)
├── keybindings/          # Keyboard actions & config
├── git/                  # Git status, diff, worktree
├── theme/                # Theming system (built-in + custom)
├── ui/                   # Shared UI utilities
├── remote/               # Remote control server (HTTP/WS API)
└── updater/              # Self-update system
```

## Architecture

### View Hierarchy

```
RootView (views/root/)
├── TitleBar (views/chrome/)
├── Sidebar (views/panels/sidebar/)
├── ProjectColumn (views/panels/project_column.rs)
│   └── LayoutContainer → TerminalPane / SplitPane / Tabs
├── StatusBar (views/panels/status_bar.rs)
└── Overlays (views/overlays/) — managed by OverlayManager
```

See `src/views/CLAUDE.md` for full hierarchy and file inventory.

### Layout System

Terminals are organized in a recursive tree structure (`LayoutNode`):
- **Terminal** — single terminal pane
- **Split** — horizontal/vertical split with children and ratios
- **Tabs** — tabbed container with multiple children

Path-based navigation: `Vec<usize>` indexes into the tree.

### GPUI Entities

Observable state with auto-notify:
- `Workspace` — projects, layouts, focus (via FocusManager)
- `RequestBroker` — decoupled transient UI request routing (overlay/sidebar requests)
- `SettingsState` — user preferences with debounced auto-save
- `AppTheme` — current theme mode and colors
- `RootView` — main view, owns SidebarController + OverlayManager
- `OverlayManager` — centralized modal overlay lifecycle
- `Sidebar` — sidebar project list with drag-and-drop

### Event Flow

1. **PTY events**: `PtyManager` → `async_channel` → `Okena` → `Terminal` (+ `PtyBroadcaster` for remote clients)
2. **UI requests**: `RequestBroker` → `cx.notify()` → observers in RootView/Sidebar
3. **State mutations**: `Workspace` notify → observers update UI
4. **Persistence**: debounced 500ms save to disk

### Configuration Files

Located in `~/.config/okena/`:
- `workspace.json` — projects, layouts, terminal state
- `settings.json` — font, theme, shell, session backend
- `keybindings.json` — custom keyboard shortcuts
- `themes/*.json` — custom theme files
- `remote.json` — remote server discovery (auto-generated)

## Platform Support

### Linux
- Standard shells: bash, zsh, fish, sh (auto-detected)
- Session backends: tmux, screen (optional)
- `simple_root.rs` — Wayland maximize workaround

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

- **gpui** + **gpui-component** — UI framework (from Zed)
- **alacritty_terminal** — Terminal emulation
- **portable-pty** — Cross-platform PTY
- **smol** + **async-channel** — Async runtime for PTY threads
- **tokio** + **axum** — Remote control server
- **serde** / **serde_json** — Serialization
- **syntect** — Syntax highlighting
- **reqwest** + **semver** — Update checker

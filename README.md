# Term Manager

A modern terminal multiplexer written in Rust, built with [GPUI](https://github.com/zed-industries/zed/tree/main/crates/gpui) (the UI framework from Zed editor).

## Installation

### macOS

**Homebrew (recommended):**

```bash
brew tap contember/term-manager
brew install --cask term-manager
```

**Or install script:**

```bash
curl -fsSL https://raw.githubusercontent.com/contember/term-manager/main/install.sh | bash
```

### Linux

```bash
curl -fsSL https://raw.githubusercontent.com/contember/term-manager/main/install.sh | bash
```

Installs to `~/.local/bin/term-manager` with desktop entry and icons.

### Windows

**PowerShell:**

```powershell
irm https://raw.githubusercontent.com/contember/term-manager/main/install.ps1 | iex
```

Installs to `%LOCALAPPDATA%\Programs\Term Manager` with Start Menu shortcut.

### Manual Download

Download from the [Releases](https://github.com/contember/term-manager/releases) page or get development builds:

| Platform | Download |
|----------|----------|
| macOS (Apple Silicon) | [term-manager-macos-arm64.zip](https://nightly.link/contember/term-manager/workflows/build/main/term-manager-macos-arm64.zip) |
| Linux (x64) | [term-manager-linux-x64.zip](https://nightly.link/contember/term-manager/workflows/build/main/term-manager-linux-x64.zip) |
| Windows (x64) | [term-manager-windows-x64.zip](https://nightly.link/contember/term-manager/workflows/build/main/term-manager-windows-x64.zip) |

## Features

- **Split panes** - Horizontal and vertical splits with drag-to-resize
- **Tabs** - Organize terminals in tab containers
- **Detachable windows** - Pop out terminals to separate windows
- **Fullscreen mode** - Focus on a single terminal
- **Minimize/restore** - Collapse terminals to save space
- **Search** - Find text with highlighting
- **Themes** - Dark/light mode with system appearance detection
- **Command palette** - Quick access to commands (Cmd+Shift+P)
- **Workspace persistence** - Auto-saves layout to JSON

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
| Command palette | Cmd+Shift+P | Ctrl+Shift+P |
| Copy | Cmd+C | Ctrl+C |
| Paste | Cmd+V | Ctrl+V |
| Find | Cmd+F | Ctrl+F |

## Dependencies

- **GPUI** + **gpui-component** - UI framework
- **alacritty_terminal** - Terminal emulation
- **portable-pty** - PTY management
- **smol** - Async runtime

## License

MIT

# Okena - Development Notes

Cross-platform terminal multiplexer built with Rust and GPUI (from Zed editor).

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
src/            # Desktop app (Rust + GPUI)
crates/         # Shared crates (okena-core)
mobile/         # Mobile app (Flutter + Rust FFI)
web/            # Web client
assets/         # Fonts, icons (assets/icons/*.svg referenced as icons/*.svg)
scripts/        # Build & utility scripts
macos/          # macOS-specific resources
Casks/          # Homebrew cask definition
docs/           # Documentation
```

Detailed documentation lives in `src/CLAUDE.md` and `src/*/CLAUDE.md` files.

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

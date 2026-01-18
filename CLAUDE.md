# Term Manager - Development Notes

## Building on Windows

### Prerequisites

1. **Visual Studio 2022** with:
   - MSVC v143 - VS 2022 C++ x64/x86 build tools
   - **Windows 10/11 SDK** (e.g., `Windows 10 SDK (10.0.22621.0)`)

2. **Rust** toolchain (via rustup)

### Important: PATH Conflict

Git for Windows includes `C:\Program Files\Git\usr\bin\link.exe` (GNU coreutils) which conflicts with MSVC's `link.exe`. If you see errors like:
- `link: extra operand '...'`
- `Try 'link --help' for more information`

**Solution:** Build from **x64 Native Tools Command Prompt for VS 2022** which sets up correct PATH.

### Build Commands

From x64 Native Tools Command Prompt:
```cmd
cd C:\Users\matej\Documents\GitHub\term-manager
cargo build
cargo run
```

Or use vcvars64.bat to set up environment:
```cmd
call "C:\Program Files\Microsoft Visual Studio\2022\Community\VC\Auxiliary\Build\vcvars64.bat"
cargo build
```

### Building from Claude Code (Bash tool)

Use PowerShell to run cargo commands:
```bash
powershell.exe -Command "Set-Location 'C:\Users\matej\Documents\GitHub\term-manager'; & 'C:\Users\matej\.cargo\bin\cargo.exe' check 2>&1"
```

For build:
```bash
powershell.exe -Command "Set-Location 'C:\Users\matej\Documents\GitHub\term-manager'; & 'C:\Users\matej\.cargo\bin\cargo.exe' build 2>&1"
```

Note: Exit code 1 from PowerShell is normal if there are warnings - check for "Finished" in output to confirm success.

### Common Issues

| Error | Cause | Solution |
|-------|-------|----------|
| `LNK1181: cannot open kernel32.lib` | Windows SDK not installed | Install via VS Installer |
| `link: extra operand` | Wrong link.exe (Git) | Use VS Developer Command Prompt |

## Architecture Notes

### Shell Configuration (Windows Support)

- `src/terminal/shell_config.rs` - ShellType enum with Windows shells (cmd, PowerShell, WSL)
- Session backends (tmux/screen) are Unix-only, disabled on Windows via `#[cfg]`
- Default shell is configurable in Settings panel and persisted in `settings.json`

### Key Files

- `src/terminal/pty_manager.rs` - PTY management, uses shell config
- `src/terminal/session_backend.rs` - tmux/screen support (Unix only)
- `src/views/settings_panel.rs` - Settings UI including shell dropdown

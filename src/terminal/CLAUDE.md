# terminal/ — Terminal Emulation & PTY Management

Wraps `alacritty_terminal` for ANSI processing and `portable-pty` for cross-platform PTY handling.

## Files

| File | Purpose |
|------|---------|
| `mod.rs` | Module re-exports. |
| `terminal.rs` | `Terminal` struct wrapping `alacritty_terminal::Term`. `Arc<Mutex>` for thread safety. Selection, search, scrollback, resize, URL detection. |
| `pty_manager.rs` | `PtyManager` — PTY lifecycle management. `PtyHandle` per terminal. Spawns OS reader/writer threads. `PtyShutdownState` for orderly cleanup. |
| `backend.rs` | Terminal backend abstraction. |
| `shell_config.rs` | `ShellType` enum, `CommandBuilder` construction. Cross-platform shell detection (bash/zsh/fish/sh on Unix; cmd/PowerShell/WSL on Windows). |
| `session_backend.rs` | `SessionBackend` enum — tmux/screen/dtach integration (Unix only). |
| `input.rs` | Key-to-bytes conversion. DECCKM cursor mode handling. Platform-specific modifier mappings. |

## Key Patterns

- **Thread model**: Each PTY gets a dedicated reader thread and writer thread (OS threads via `smol`), communicating with the GPUI thread via `async_channel`.
- **Locking**: `Terminal` internals are behind `Arc<Mutex>` since the reader thread and GPUI thread both need access.
- **Shell detection**: Auto-detects available shells on the system. On Windows, detects WSL distros and converts paths (`C:\` → `/mnt/c/`).
